[English](./product-terminology.md) | [한국어](./product-terminology.ko.md)

# 제품 용어와 고위험 문구

이 문서는 Maekon 사용자 노출 용어의 SSOT다. 개인정보, 동의, 삭제,
제공자·AI 외부 전송, 연동, 자동화, 제안, 업데이트 복구 문구를 다룬다.
한국어 표현을 제품 언어의 기준으로 삼고, 안전 의미가 바뀌는 수정은 지원하는
5개 locale 모두에서 같은 의미를 유지해야 한다.

## 작성 원칙

1. 사용자가 하는 행동과 관찰 가능한 결과를 먼저 쓴다. 평이한 제품 용어로
   정확히 설명할 수 있으면 법률·내부 구현 용어를 앞세우지 않는다.
2. 앞으로의 수집 중단과 이미 수집된 데이터 삭제를 구분한다.
3. 파괴적 확인창은 대상, 기기·저장 범위, 결과, Maekon 밖에 남는 파일을
   함께 명시한다.
4. best-effort 요청을 삭제 완료처럼 표현하지 않는다. 현재 동의 철회 명령은
   로컬 DB와 프레임 파일 삭제가 끝나야 성공하며, 실패는 사용자에게 남긴다.
5. 사용자에게는 현지화된 복구 행동을 먼저 보여 준다. 제공자·HTTP·파서·OS의
   원문 오류는 접힌 `기술 세부 정보` 안에 둔다.
6. 경로·탐색 라벨은 실제로 존재하는 화면을 설명해야 한다.

## 권장 용어

| 개념 | English | 한국어 기준 | 日本語 | 简体中文 | Español |
| --- | --- | --- | --- | --- | --- |
| 동의 철회와 앱 관리 데이터 삭제 | Withdraw consent and delete data | 동의 철회 및 데이터 삭제 | 同意の撤回とデータ削除 | 撤回同意并删除数据 | Retirar el consentimiento y eliminar datos |
| 앞으로의 수집만 중단 | Turn monitoring off | 모니터링 끄기 | モニタリングをオフ | 关闭监控 | Desactivar la monitorización |
| 앱 관리 데이터 전체 삭제 | Delete all data | 모든 데이터 삭제 | すべてのデータを削除 | 删除所有数据 | Eliminar todos los datos |
| 받을 릴리스 계열 선택 | Update channel | 업데이트 채널 | アップデートチャネル | 更新频道 | Canal de actualización |
| 원문 실패 내용 펼치기 | Technical details | 기술 세부 정보 | 技術的な詳細 | 技术详情 | Detalles técnicos |
| 기기 밖으로 나가는 데이터 | External transfer | 외부 전송 | 外部送信 | 外部传输 | Transferencia externa |
| 외부 전송 데이터를 처리하는 서비스 | Provider | 제공자 | プロバイダー | 提供商 | Proveedor |

제품 행동에는 `삭제`를 쓴다. `소거`와 법률 조문 표현은 구분이 필요한 정책,
감사, 엔지니어링 문서에만 사용한다.

## 고위험 인벤토리와 결정

| 화면 | 리소스·경로 | 발견한 위험 | 결정 |
| --- | --- | --- | --- |
| 동의 철회 | `privacy.consent.withdraw.*` | `소거`가 부자연스럽고 행동 범위가 암묵적이었음 | 삭제를 직접 명명하고, 이 기기에서 Maekon이 관리하는 데이터가 대상임을 밝히며, 내보내기·백업 파일 예외를 유지 |
| 업데이트 탐색 | `sidebar.updateChannel`, `/updates/channel` | 실제 화면은 채널 선택인데 `업데이트 기록`을 약속함 | 모든 locale에서 `업데이트 채널` 의미로 통일 |
| 업데이트 실패 | `updates.statusCheckFailed`, `updates.actionFailed` | HTTP·파서 원문이 사용자 메시지의 전면에 노출될 수 있었음 | 현지화된 복구 문구를 먼저 표시하고 원문은 접힌 기술 세부 정보에만 유지 |
| 설치 행동 | `updates.readyToInstallMsg`, `updates.installNow` | 한국어·중국어·스페인어 리소스가 영어 fallback을 유지함 | 두 문구를 지원하는 모든 locale에서 현지화 |
| 제공자·AI 외부 전송 | `privacy.consent.microphone.*`, `privacy.consent.unredactedExternalOcr.*` | 문구 수정 때 안전 의미가 어긋날 수 있음 | 제공자, payload, opt-in, 이미 전송된 데이터 범위를 명시하고 5개 locale key parity test를 요구 |

## 검증 계약

- `product-copy-parity.test.ts`가 5개 locale의 삭제, 외부 파일 예외,
  업데이트 채널, 복구 문구, 설치 행동을 고정한다.
- `UpdatePanel.test.tsx`가 updater 원문 실패가 현지화된 복구 문구 아래의
  접힌 세부 정보로만 표시되는지 검증한다.
- 레이아웃에 영향을 줄 수 있는 문구는 병합 전에 동의 확인창과 업데이트
  상태·채널 화면을 1024×768, 1280×800에서 확인한다.
- UI 문자열은 locale 리소스에만 둔다. 컴포넌트나 selector에 locale별
  문자열을 하드코딩하지 않는다.

파괴 범위나 법적 의미가 바뀌는 변경은 제품 검토 외에 개인정보·법률 검토가
필요하다. 이 계약을 유지하는 번역 정리도 5개 locale parity 검증을 거친다.
