//! 디스크 사용량 표본을 SQLite 에 5분·1시간·1일 해상도로 보존한다.
//!
//! 표 하나에 해상도 열을 두고, 기록할 때마다 같은 트랜잭션 안에서
//! 5분 버킷 갱신 → 시간 집계 → 일 집계 → 보존 기한 정리를 끝낸다. 재집계는 소스가
//! 온전히 남아 있는 버킷만 다시 세므로 몇 번을 돌려도 같은 값이 나온다.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::application::HistoryRepository;
use crate::domain::disk::{Bucket, Resolution, Sample, rollup};

const DATABASE_FILE: &str = "history.sqlite3";
const DATABASE_VERSION: i64 = 1;

#[derive(Debug, Clone)]
pub(crate) struct SqliteHistoryRepository {
    database: PathBuf,
}

impl SqliteHistoryRepository {
    pub(crate) fn production() -> Result<Self> {
        let home = std::env::var_os("HOME").context("HOME is not set")?;
        Ok(Self::at(
            PathBuf::from(home)
                .join(".cache/diskmeter")
                .join(DATABASE_FILE),
        ))
    }

    pub(crate) fn at(database: PathBuf) -> Self {
        Self { database }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.database
    }

    fn open(&self) -> Result<Connection> {
        if let Some(parent) = self.database.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        let connection = Connection::open(&self.database)
            .with_context(|| format!("could not open {}", self.database.display()))?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .context("could not set the SQLite busy timeout")?;
        // 상주 프로세스가 쓰는 동안 CLI 가 읽어도 서로 막지 않게 한다. 실패해도 동작은 같다.
        let _ = connection.pragma_update(None, "journal_mode", "WAL");
        initialize(&connection)?;
        Ok(connection)
    }
}

fn initialize(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS history (
                 resolution     TEXT    NOT NULL,
                 path           TEXT    NOT NULL,
                 bucket         INTEGER NOT NULL,
                 total          INTEGER NOT NULL,
                 used_min       INTEGER NOT NULL,
                 used_max       INTEGER NOT NULL,
                 used_sum       INTEGER NOT NULL,
                 used_last      INTEGER NOT NULL,
                 available_last INTEGER NOT NULL,
                 samples        INTEGER NOT NULL,
                 PRIMARY KEY (resolution, path, bucket)
             );",
        )
        .context("could not create the history schema")?;
    let version: Option<i64> = connection
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
            row.get(0)
        })
        .optional()
        .context("could not read the history schema version")?;
    match version {
        None => {
            connection
                .execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    [DATABASE_VERSION],
                )
                .context("could not record the history schema version")?;
        }
        Some(found) if found == DATABASE_VERSION => {}
        Some(found) => {
            bail!("the history database is version {found}, this build expects {DATABASE_VERSION}")
        }
    }
    Ok(())
}

impl HistoryRepository for SqliteHistoryRepository {
    fn record(&self, path: &str, sample: Sample) -> Result<()> {
        let mut connection = self.open()?;
        let tx = connection
            .transaction()
            .context("could not start the history transaction")?;

        let fine = Resolution::FiveMinutes;
        let start = fine.bucket_start(sample.at);
        let incoming = Bucket::from_usage(start, sample.usage);
        let bucket = match read_bucket(&tx, path, fine, start)? {
            Some(mut existing) => {
                existing.absorb(&incoming);
                existing
            }
            None => incoming,
        };
        upsert(&tx, path, fine, &bucket)?;
        for resolution in [Resolution::Hourly, Resolution::Daily] {
            rollup_into(&tx, path, resolution, sample.at)?;
        }
        prune(&tx, sample.at)?;

        tx.commit()
            .context("could not commit the history transaction")
    }

    fn series(
        &self,
        path: &str,
        resolution: Resolution,
        from: i64,
        to: i64,
    ) -> Result<Vec<Bucket>> {
        let connection = self.open()?;
        select(&connection, path, resolution, from, to)
    }
}

/// 소스 해상도의 버킷으로 `resolution` 버킷을 다시 센다.
///
/// 소스 행은 보존 기한이 지나면 지워지므로, 기한이 걸친 버킷은 앞쪽이 잘렸을 수
/// 있다. 그래서 기한 이후 첫 경계부터만 다시 세고, 그 앞의 버킷은 소스가 온전하던
/// 때 센 값이 그대로 남는다. 집계가 정리보다 먼저 실행되므로 그 값은 완전하다.
fn rollup_into(
    tx: &Transaction,
    path: &str,
    resolution: Resolution,
    now: DateTime<Local>,
) -> Result<()> {
    let Some(source) = resolution.source() else {
        return Ok(());
    };
    let cutoff = (now - source.retention()).timestamp();
    let from = resolution.first_boundary_at_or_after(cutoff);
    let rows = select(tx, path, source, from, i64::MAX)?;
    for bucket in rollup(resolution, &rows) {
        upsert(tx, path, resolution, &bucket)?;
    }
    Ok(())
}

fn prune(tx: &Transaction, now: DateTime<Local>) -> Result<()> {
    for resolution in Resolution::ALL {
        let cutoff = (now - resolution.retention()).timestamp();
        tx.execute(
            "DELETE FROM history WHERE resolution = ?1 AND bucket < ?2",
            params![resolution.name(), cutoff],
        )
        .with_context(|| format!("could not prune the {} history", resolution.name()))?;
    }
    Ok(())
}

const COLUMNS: &str =
    "bucket, total, used_min, used_max, used_sum, used_last, available_last, samples";

/// SQLite INTEGER 는 부호 있는 64비트다. 바이트 수는 그 안에 들어가지만 타입은 맞춰 줘야 한다.
fn stored(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn loaded(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn row_to_bucket(row: &rusqlite::Row<'_>) -> rusqlite::Result<Bucket> {
    Ok(Bucket {
        start: row.get(0)?,
        total: loaded(row.get(1)?),
        used_min: loaded(row.get(2)?),
        used_max: loaded(row.get(3)?),
        used_sum: loaded(row.get(4)?),
        used_last: loaded(row.get(5)?),
        available_last: loaded(row.get(6)?),
        samples: u32::try_from(row.get::<_, i64>(7)?).unwrap_or(u32::MAX),
    })
}

fn read_bucket(
    connection: &Connection,
    path: &str,
    resolution: Resolution,
    start: i64,
) -> Result<Option<Bucket>> {
    connection
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM history WHERE resolution = ?1 AND path = ?2 AND bucket = ?3"
            ),
            params![resolution.name(), path, start],
            row_to_bucket,
        )
        .optional()
        .context("could not read a history bucket")
}

fn select(
    connection: &Connection,
    path: &str,
    resolution: Resolution,
    from: i64,
    to: i64,
) -> Result<Vec<Bucket>> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT {COLUMNS} FROM history
             WHERE resolution = ?1 AND path = ?2 AND bucket >= ?3 AND bucket <= ?4
             ORDER BY bucket"
        ))
        .context("could not prepare the history query")?;
    let rows = statement
        .query_map(params![resolution.name(), path, from, to], row_to_bucket)
        .context("could not run the history query")?;
    rows.collect::<rusqlite::Result<Vec<Bucket>>>()
        .context("could not read the history rows")
}

fn upsert(
    connection: &Connection,
    path: &str,
    resolution: Resolution,
    bucket: &Bucket,
) -> Result<()> {
    connection
        .execute(
            "INSERT INTO history (resolution, path, bucket, total, used_min, used_max, used_sum,
                                  used_last, available_last, samples)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT (resolution, path, bucket) DO UPDATE SET
                 total = excluded.total,
                 used_min = excluded.used_min,
                 used_max = excluded.used_max,
                 used_sum = excluded.used_sum,
                 used_last = excluded.used_last,
                 available_last = excluded.available_last,
                 samples = excluded.samples",
            params![
                resolution.name(),
                path,
                bucket.start,
                stored(bucket.total),
                stored(bucket.used_min),
                stored(bucket.used_max),
                stored(bucket.used_sum),
                stored(bucket.used_last),
                stored(bucket.available_last),
                i64::from(bucket.samples),
            ],
        )
        .with_context(|| format!("could not store a {} bucket", resolution.name()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use chrono::{TimeDelta, TimeZone};

    use super::*;
    use crate::domain::disk::DiskUsage;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn repository() -> SqliteHistoryRepository {
        let unique = format!(
            "diskmeter-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        );
        SqliteHistoryRepository::at(std::env::temp_dir().join(unique).join("history.sqlite3"))
    }

    fn sample(at: DateTime<Local>, used: u64) -> Sample {
        Sample {
            at,
            usage: DiskUsage::new(1_000, used, 1_000 - used),
        }
    }

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap()
    }

    fn all(repository: &SqliteHistoryRepository, resolution: Resolution) -> Vec<Bucket> {
        repository
            .series("/", resolution, i64::MIN, i64::MAX)
            .unwrap()
    }

    #[test]
    fn samples_in_one_five_minute_bucket_merge_into_one_row() {
        let repository = repository();
        let base = Local::now();
        repository.record("/", sample(base, 500)).unwrap();
        repository
            .record("/", sample(base + TimeDelta::seconds(1), 700))
            .unwrap();
        let fine = all(&repository, Resolution::FiveMinutes);
        // 두 표본이 5분 경계에 걸치면 두 행이지만, 어느 쪽이든 합은 보존된다.
        let samples: u32 = fine.iter().map(|bucket| bucket.samples).sum();
        assert_eq!(samples, 2);
        let hourly = all(&repository, Resolution::Hourly);
        assert_eq!(hourly.iter().map(|b| b.samples).sum::<u32>(), 2);
        assert_eq!(hourly.last().unwrap().used_last, 700);
        assert_eq!(hourly.iter().map(|b| b.used_min).min(), Some(500));
        let daily = all(&repository, Resolution::Daily);
        assert_eq!(daily.iter().map(|b| b.samples).sum::<u32>(), 2);
    }

    #[test]
    fn hourly_and_daily_rows_follow_their_own_grids() {
        let repository = repository();
        repository
            .record("/", sample(at(2026, 10, 5, 23, 50), 100))
            .unwrap();
        repository
            .record("/", sample(at(2026, 10, 6, 0, 10), 200))
            .unwrap();
        repository
            .record("/", sample(at(2026, 10, 6, 0, 40), 300))
            .unwrap();
        let hourly = all(&repository, Resolution::Hourly);
        assert_eq!(hourly.len(), 2, "{hourly:?}");
        assert_eq!(hourly[1].samples, 2);
        assert_eq!(hourly[1].used_avg(), 250);
        let daily = all(&repository, Resolution::Daily);
        assert_eq!(daily.len(), 2, "{daily:?}");
        assert_eq!(daily[0].start, at(2026, 10, 5, 0, 0).timestamp());
        assert_eq!(daily[1].start, at(2026, 10, 6, 0, 0).timestamp());
        assert_eq!(daily[1].used_last, 300);
    }

    #[test]
    fn pruning_drops_only_the_fine_rows_and_freezes_their_rollups() {
        let repository = repository();
        let now = Local::now();
        let old = now - TimeDelta::days(3);
        // 세 번 기록된 옛 시간: 집계는 3표본으로 남아야 한다.
        for minutes in [0, 5, 10] {
            repository
                .record("/", sample(old + TimeDelta::minutes(minutes), 400))
                .unwrap();
        }
        assert_eq!(all(&repository, Resolution::FiveMinutes).len(), 3);

        repository.record("/", sample(now, 900)).unwrap();

        let fine = all(&repository, Resolution::FiveMinutes);
        assert_eq!(
            fine.len(),
            1,
            "48시간이 지난 5분 버킷은 지워져야 함: {fine:?}"
        );
        let hourly = all(&repository, Resolution::Hourly);
        assert_eq!(hourly.len(), 2, "{hourly:?}");
        assert_eq!(
            hourly[0].samples, 3,
            "소스가 사라진 뒤에도 옛 시간 집계는 그대로여야 함"
        );
        assert_eq!(hourly[0].used_avg(), 400);
        let daily = all(&repository, Resolution::Daily);
        assert!(daily.len() <= 2 && !daily.is_empty(), "{daily:?}");
        assert_eq!(daily.last().unwrap().used_last, 900);
    }

    #[test]
    fn series_is_bounded_and_ordered() {
        let repository = repository();
        let base = at(2026, 10, 1, 12, 0);
        for hour in [3, 1, 2] {
            repository
                .record(
                    "/",
                    sample(base + TimeDelta::hours(hour), 100 * hour as u64),
                )
                .unwrap();
        }
        let from = (base + TimeDelta::hours(2)).timestamp();
        let rows = repository
            .series("/", Resolution::Hourly, from, i64::MAX)
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].start < rows[1].start);
        assert!(
            repository
                .series("/Volumes/Other", Resolution::Hourly, 0, i64::MAX)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_newer_schema_is_refused_instead_of_being_rewritten() {
        let repository = repository();
        repository.record("/", sample(Local::now(), 1)).unwrap();
        let connection = Connection::open(repository.path()).unwrap();
        connection
            .execute("UPDATE schema_version SET version = 99", [])
            .unwrap();
        let error = repository.record("/", sample(Local::now(), 2)).unwrap_err();
        assert!(format!("{error:#}").contains("version 99"), "{error:#}");
    }
}
