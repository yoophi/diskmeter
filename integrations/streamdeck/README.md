# DiskMeter → Stream Deck

로컬 DiskMeter의 루트 디스크(`/`) 사용률을 표시하는 독립 macOS 플러그인입니다.
Stream Deck 7.6 이상과 앱에 포함된 Node.js 24를 사용하며 npm 런타임 의존성이 없습니다.
AgentMeter 플러그인이나 저장소 없이 동작합니다.

`http://127.0.0.1:9998/api/dashboard`를 30초마다 읽고, `DISK / ROOT /` 제목,
큰 사용률 %, 프로그레스 바를 표시합니다. 차트 하단의 텍스트는 숨깁니다.
70% 이상은 노란색, 90% 이상은 빨간색이며, 조회 실패 시 마지막 값은 회색으로 표시합니다.
데이터가 전혀 없으면 중앙에 상태 문구를 표시합니다. 키를 누르면 DiskMeter 대시보드가 열립니다.

## 설치

DiskMeter 웹 서버를 9998 포트에서 실행한 상태로 이 디렉터리에서:

```sh
npx --yes @elgato/cli link ./com.yoophi.diskmeter.sdPlugin
npx --yes @elgato/cli restart com.yoophi.diskmeter
```

새 액션이 보이지 않으면 Stream Deck 앱을 완전히 종료하고 다시 실행합니다.
**DiskMeter → DiskMeter Usage**를 빈 키로 드래그합니다.
기존 AgentMeter의 DiskMeter 키를 이전할 때도 이 액션으로 교체합니다.

## 검증

```sh
node --test usage.test.mjs
npx --yes @elgato/cli validate ./com.yoophi.diskmeter.sdPlugin
```
