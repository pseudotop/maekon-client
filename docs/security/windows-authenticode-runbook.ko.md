# Windows Authenticode 활성화 런북

## 현재 상태

Windows 서명 파이프라인은 `prepared_not_active` 상태다. 저장소는 필요한 산출물,
최종 바이트 순서, OIDC 주체, 게시자·타임스탬프 검증 계약을 고정하지만, 법인 Public
Trust 신원과 인증서 프로필이 준비되기 전에는 서명을 주장하지 않는다.

정본 정책은 `supply-chain/windows-authenticode-policy.json`이다. ADR-005에 따라
인증서와 외부 서명 서비스의 소유자는 DevOps/릴리스 엔지니어링이다.

## 준비 원칙

- PFX, 개인 키, 인증서 비밀번호를 저장소나 GitHub Actions secret에 넣지 않는다.
- Azure Artifact Signing Public Trust와 GitHub OIDC를 사용한다.
- OIDC 주체는
  `repo:pseudotop/maekon-client:environment:release-signing`에 정확히 고정한다.
- `Artifact Signing Certificate Profile Signer` 역할은 해당 인증서 프로필 범위에만
  부여한다.
- Public Trust 신원 확인 전에는 정책의 `enforcement_state`를 `active`로 바꾸지 않는다.
- 서명은 SHA-256과 RFC 3161 타임스탬프를 사용한다.

## 외부 준비

1. 법인 명의와 일치하는 Azure 구독 결제 프로필을 준비한다.
2. Korea Central에 Artifact Signing Basic 계정을 만든다.
3. 법인 Public Identity Validation을 완료한다.
4. `Public Trust` 인증서 프로필을 만든다.
5. Entra 애플리케이션과 위 `release-signing` 환경 전용 federated credential을 만든다.
6. 인증서 프로필 범위에 Signer 역할을 부여한다.
7. 공개 저장소의 보호된 `release-signing` 환경에 아래 값을 등록한다.

비밀 키가 아닌 환경 변수:

- `AZURE_TENANT_ID`
- `AZURE_SUBSCRIPTION_ID`
- `AZURE_CLIENT_ID`
- `WINDOWS_ARTIFACT_SIGNING_ENDPOINT`
- `WINDOWS_ARTIFACT_SIGNING_ACCOUNT`
- `WINDOWS_ARTIFACT_SIGNING_PROFILE`

이 값들은 보호된 `release-signing` 환경에 둔다. 게시자 subject와 활성화 상태는 가변 환경
변수가 아니라 `supply-chain/windows-authenticode-policy.json`의 검토 가능한 source
contract로 관리한다.

## 구현 시 보존할 최종 바이트 순서

1. `maekon.exe`와 `maekon-sandbox-worker.exe`를 서명한다.
2. 서명된 실행 파일로 ZIP, 한국어 MSI, 영어 MSI, NSIS를 만든다.
3. 두 MSI와 NSIS setup EXE를 서명한다.
4. 모든 실행 파일과 설치 파일에서 게시자·RFC 3161 타임스탬프·WinTrust를 검증한다.
5. 검증된 최종 바이트로 SHA-256을 만든다.
6. 기존 Ed25519 업데이트 서명과 provenance를 만든다.
7. 릴리스 자산을 게시한다.

Authenticode는 Windows 게시자 신뢰이고 Ed25519 `.sig`는 Maekon 업데이트 무결성이다.
어느 한쪽도 다른 쪽을 대체하지 않는다.

## 활성화 전 리허설

태그나 Release를 만들지 않는 정확한 SHA 리허설에서 다음을 증명한다.

- 서명 서비스가 없거나 OIDC 주체가 다르면 서명 단계가 실패한다.
- `scripts/verify-windows-authenticode.ps1`의 `Required` 모드가 미서명·게시자 불일치·
  타임스탬프 누락·변조된 파일을 거부한다.
- 설치 후 `maekon.exe`와 `maekon-sandbox-worker.exe`도 동일한 게시자로 서명되어 있다.
- Authenticode 이후 생성한 checksum과 Ed25519 sidecar가 최종 설치 파일 바이트와 일치한다.

리허설 증거와 유지보수자 승인이 확보된 뒤에만
보호된 Windows 서명 job을 추가하고, 정책의 `publisher_subject`를 인증서의 exact subject로
설정하며, 같은 PR에서 `enforcement_state`를 `active`로 변경한다. 활성화 후 공개 RC와 stable
릴리스는 미서명 상태로 되돌아갈 수 없다.

## 폐기와 사고 대응

- Artifact Signing 감사 로그와 GitHub run URL을 릴리스 증거에 보존한다.
- 의심되는 서명은 인증서 프로필에서 즉시 폐기하고 릴리스를 중단한다.
- 폐기 원인, 영향받은 SHA-256, 태그, 서명 시각을 기록한다.
- 새 프로필은 동일한 비게시 리허설을 통과한 뒤 활성화한다.
