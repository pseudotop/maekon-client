[English](./global-alpha-feedback-operations.md) | [한국어](./global-alpha-feedback-operations.ko.md)

# Global Alpha 피드백·프라이버시·인시던트 운영

이 문서는 초대 기반 Maekon Global Alpha 피드백의 운영 계약이다.
기계 판독 SSOT는 `docs/contracts/global-alpha-feedback-policy.json`, 공개 안내와
이메일 초안 폼의 canonical URL은 `https://maekon.dev/alpha-feedback`다.

## 현재 게이트

정책 `2026-08-23.1`의 상태는 `hold`다. #8683의 이전 저장소 전역 HOLD는
해제되었으며 현재 보류 사유가 아니다. #8685의 릴리스 신뢰 잔여가 닫히고,
#8687 측정 계약이 준비되며, 지정 운영자가 현재 응답 역량을 확인하기 전까지
신규 초대, 일반 Alpha 접수, 예약 Alpha 게시를 허용하지 않는다. 프라이버시
요청, 참여 철회, 비공개 취약점 신고는 계속 연다. 현재 커뮤니티는 열거나
홍보하지 않는다. `intake_state` 변경은 검토된 manifest 수정이 필요하며 이
진입 게이트를 우회할 수 없다.

## 경로 분리

| 요청 | 경로 | 공개 이슈 허용 |
|---|---|---|
| 초대 기반 Alpha 피드백 | `support@maekon.dev`, 제목 `[Maekon Alpha Feedback]` | 참여자·진단 데이터 금지 |
| 프라이버시·열람·정정·삭제 | `support@maekon.dev`, 제목 `[Maekon Alpha Privacy Request]` | 금지 |
| 참여 철회 | `support@maekon.dev`, 제목 `[Maekon Alpha Withdrawal]` | 금지 |
| 보안 취약점 | GitHub 비공개 취약점 신고 | 항상 금지 |

공개 페이지는 네트워크 요청을 보내거나 폼 값을 저장하지 않는다. 사용자가
검토할 수 있는 로컬 `mailto:` 초안만 만든다. 초안을 열거나 보내는 것은
접수증이 아니며, 아래 지원팀 회신이 접수증이다.

## 접수 경계

수집 필드는 요청 유형, 연락 이메일, 유입 경로, OS, 정확한 버전·커밋,
합성 요약, 참여 동의, 별도 인용 동의, 진단 첨부 opt-in, 정책 버전, UTC 제출
시각이다.

다음 항목은 요청하거나 받지 않는다.

- 원본 화면, 스크린샷, OCR 텍스트, 창 제목
- 프롬프트, 대화 내용, 비밀정보, 자격증명, 고객 데이터
- 다른 사람의 이메일, 전체 로컬 경로
- 자동 업로드된 진단 번들

페이지에는 파일 입력이 없다. 진단이 필요하면 참여자가 Maekon **지원 및
진단**에서 번들을 직접 생성하고 내보낸 내용을 검토한 뒤 수동으로 첨부해야
한다. 진단 opt-in은 인용, 연구, 제품, 텔레메트리 동의를 뜻하지 않는다.

## 접수증과 운영 목표

운영자는 영업일 3일 이내에 지원 메일함에서 다음 최소 접수증을 회신한다.

```text
MAEKON-ALPHA-RECEIPT
receipt_id: <opaque id>
received_at: <UTC timestamp>
request_type: <feedback|privacy|withdrawal>
policy_version: 2026-08-23.1
target_by: <UTC date>
```

프라이버시·철회 완료 목표는 검증된 접수 뒤 30일이다. 치명적 설치 실패,
crash loop, 데이터 손실, 프라이버시 경계 불일치는 24시간 안에 분류한다.
이는 운영 목표이며 특정 요청의 접수·완료 증거가 아니다.

## 저장과 접근

- 데이터 소유자와 등록된 유일한 메일함 운영자: `pseudotop`
- 일반 피드백 보존: 최대 90일
- 종료된 연락 기록과 opt-in 진단 첨부물: 최대 30일
- 검증된 조기 삭제 요청이 있으면 해당 요청을 우선
- 사용자 내용, 직접 식별자, 메일 본문, 진단 첨부물을 GitHub 이슈,
  Project 필드, outreach log, D7 readback에 복사하지 않음

#8688과 #8697에는 프라이버시 안전 집계 count만 전달할 수 있다. count에는
denominator, 관측 기간, query/export receipt가 있어야 한다.

## 일일 분류

접수 활성 기간에 `pseudotop` 운영자가 하루 한 번 다음을 수행한다.

1. 새 메시지를 피드백, 프라이버시, 철회, 보안으로 분류한다.
2. 보안 신고는 exploit 내용을 공개 표면에 복사하지 않고 비공개 취약점
   경로로 전환한다.
3. timestamped receipt를 보내고 목표 날짜를 지정한다.
4. 첨부물이 명시적으로 opt-in·검토됐는지 확인하고 예상치 못한 원본 내용은
   거절·삭제한다.
5. 비공개 운영 대장에는 opaque receipt ID, 상태, 정책 버전, severity만 적는다.
6. #8688에는 집계 outreach 상태만 반영하고 D7 집계는 #8697로 전달한다.
   참여자 단위 내용은 게시하지 않는다.

## 철회와 삭제

검증된 철회 뒤에는 앞으로의 Alpha 연락과 참여자 단위 측정을 즉시 중단하고,
직접 식별자와 보관 첨부물을 제거한 뒤 완료 회신을 보낸다. 이전에 계산한
재식별 불가능 집계 count는 남을 수 있지만, 연락 재개, 참여자 기록 복원,
retention·customer 증거에 사용할 수 없다.

제품 로컬 Maekon 데이터와 Alpha 연락 기록은 서로 다르다. Alpha 참여 철회만
으로 기기 로컬 데이터가 삭제되지는 않으므로 해당 범위는 Maekon의 로컬 삭제
제어를 사용해야 한다. 반대로 로컬 삭제는 별도로 보낸 지원 메일 삭제 증거가
아니다.

## Fail-closed 인시던트 중단

치명적 설치 실패, crash loop, 데이터 손실, 프라이버시 경계 불일치가 확인되면:

1. 프라이버시·철회·비공개 보안 경로는 계속 연다.
2. 신규 초대와 예약 게시를 중단한다.
3. 검토된 변경으로 정책 manifest를 `paused`로 바꾼다.
4. 참여자 내용을 제외한 전용 인시던트 이슈를 생성하거나 연결하고 owner를 기록한다.
5. 일반 접수 재개 전에 새 릴리스 결정을 요구한다.

피드백, 인터뷰, 진단, 등록, 접수증은 customer, revenue, retention,
product-value, stable-release 증거가 아니다.
