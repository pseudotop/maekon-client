[English](./ADR-012-adaptive-tiered-memory.md) | [한국어](./ADR-012-adaptive-tiered-memory.ko.md)

# ADR-012: 적응형 계층 메모리

| 필드 | 값 |
|------|---|
| 상태 | Accepted |
| 날짜 | 2026-03-18 |
| 범위 | AdaptiveTrigger, CalibrationStore, RegimeDetector/Classifier/Manager, SegmentSummarizer, Content-Level Detection, WorkType Classification |

## 컨텍스트

분석 파이프라인(ADR-011)은 제안을 생성하지만 시간적 컨텍스트가 부족하다 — 시간에 따른 작업 세그먼트나 행동 패턴을 이해하지 못하고 시점 스냅샷을 분석한다. 고정 간격 요약(시간별/일별)은 정보를 잃는다: 90분 심층 작업 세션이 경계에 걸쳐 분할되고, 혼란스러운 10분 기간이 조용한 시간 안에 희석된다.

정보 밀도로 구동되는 적응형 세그먼테이션이 필요하며, 작업 모드별로 파라미터를 최적화하는 자동 발견된 활동 레짐이 필요하다.

## 결정 사항

### §1 Dual-EWMA 트리거를 통한 적응형 세그먼테이션

세그먼트 경계는 벽시계 간격이 아니라 트리거 점수 함수로 결정된다. 네 가지 신호(밀도, 중요도, 컨텍스트 변화, 버퍼 압력)가 설정 가능한 가중치로 결합된다. 히스테리시스 게이트(T_high/T_low)가 진동을 방지한다.

AdaptiveTrigger는 `maekon-analysis`의 **순수 알고리즘** — I/O 의존성 없음. `TriggerInput` 이벤트를 수신하고 세그먼트 라이프사이클 액션을 발생시킨다.

최소 세그먼트 지속 시간: 120초 (무의미한 마이크로 세그먼트 방지).
최대 세그먼트 지속 시간: 600초 (강제 요약을 위한 하드 백스톱).

### §2 CalibrationStore — 버퍼링된 동기 쓰기 + 비동기 읽기

`maekon-core`의 두 port trait:
- `CalibrationWriter: Send + Sync` — `CalibrationBuffer`를 통한 **동기** 배치 쓰기 (10항목 또는 5초마다 flush). 핫 경로 레이턴시를 피하기 위해 이벤트별 동기화 없음.
- `CalibrationReader: Send + Sync + #[async_trait]` — 레짐 감지를 위한 비동기 bulk 읽기.

모든 트리거 입력은 소급 재보정, 노이즈 제외, 레짐 재학습을 위해 영구 저장된다. 보존: 30일 또는 500,000행 (ring buffer 백스톱).

파라미터 스냅샷은 정규화된다: `params_version_id`가 행별 JSON 팽창을 피하기 위해 별도 `trigger_params_snapshots` 테이블을 참조.

### §3 자동 발견 레짐

레짐(활동 모드)은 수동 정의가 아니라 클러스터링을 통해 **자동 발견**된다. 프리셋 프로파일(Developer, Manager 등)은 학습된 레짐으로 대체되는 Day 1 시드 역할을 한다.

**RegimeDetector**: 수작업으로 구현된 k-means 클러스터링(외부 의존성 없음). 5개 feature, 최대 7개 클러스터, 최적 k 선택을 위한 실루엣 점수. 매일 또는 요청 시 실행.

**RegimeClassifier**: 5분 슬라이딩 윈도우에서의 실시간 nearest-centroid 매칭. 레짐 전환 시 AdaptiveTrigger 파라미터를 전환.

**RegimeManager**: 라이프사이클 규칙 — 생성(≥50 샘플), 병합(유사 centroid), 비활성화(14일 미등장), 보관(30일 미활성), 제한(최대 7개 활성).

### §4 파라미터 계층 (CSS Cascade)

파라미터는 특이성 기반 오버라이드로 해결된다:

```
Level 0: 전역 기본값 (ResolvedParams::default())
Level 1: 레짐 오버라이드 (Option 필드 — Some 값만 오버라이드)
Level 2: 카테고리 오버라이드 (AppCategory별)
Level 3: 프로세스 오버라이드 (앱 이름별)
```

`TriggerParams`는 cascade 모델을 위해 `Option<f32>` 필드를 사용. `ResolvedParams`는 완전히 해결된(Options 없음) 출력. 가중치는 합이 1.0이 되도록 자동 정규화.

### §5 컨텐츠 레벨 활동 감지

각 앱 내에서 사용자가 작업하는 **컨텐츠**를 OCR 기반으로 감지 — RDP/VM 컨테이너뿐만 아니라 보편적으로.

**TitleBarParser**: 윈도우 제목에서 컨텐츠를 추출하는 앱별 설정 가능한 정규식 규칙. VSCode, Chrome, Slack, Terminal, IntelliJ, Figma 등의 알려진 패턴.

**컨테이너 감지**: RDP/VM/VNC/Citrix 앱의 프리셋 목록. 활성 앱이 컨테이너인 경우 OCR이 서브프로세스 감지를 위해 내부 제목 표시줄을 파싱.

**ContentTracker**: 각 세그먼트 내 컨텐츠별 지속 시간을 누적.

### §6 WorkType 분류

키보드/마우스의 입력 활동 패턴(`InputActivityCollector`에서)이 OCR 컨텐츠와 상관되어 **작업 유형**을 분류: ActiveCoding, CodeReview, Writing, Reading, Designing, FormFilling, PassiveMeeting 등.

**WorkTypeClassifier**: `maekon-analysis`의 순수 알고리즘. `(KeyboardActivity, MouseActivity, content_label, app_category)` → `WorkType`를 받음.

WorkType 전환은 AdaptiveTrigger에서 중요한 이벤트다.

### §7 동의 및 프라이버시

CalibrationStore는 `ConsentManager`(`activity_pattern_learning` 권한)를 통한 명시적 동의가 필요. `TieredMemoryConfig.enabled`와 동의 모두 true여야 한다.

### §8 노이즈 처리

- 짧은 이상(1시간 미만, 레짐 매칭 없음): 노이즈로 표시, 학습에서 제외
- 지속적 변화(24시간 이상): 레짐 재감지 트리거
- 소급 재보정: 사용자가 시간 범위를 노이즈로 표시 → 레짐 파라미터 재계산
- 모든 저장된 입력은 롤백과 재학습을 가능하게 함

### §9 ContextAnalyzer와의 통합

AdaptiveTrigger와 ContextAnalyzer는 공존한다:
- AdaptiveTrigger: **언제** 세그먼트화할지 (신호 기반)
- ContextAnalyzer: **무엇을** 제안할지 (LLM 기반)

통합: 더 풍부한 LLM 컨텍스트를 위해 현재 세그먼트 통계 + 레짐 정보가 ContextAssembler에 입력. 레짐 인식 제안 필터링(Deep Focus 중 낮은 우선순위 억제, Communication 중 협업 부스트).

## 결과

- `maekon-analysis`가 다음으로 성장: AdaptiveTrigger, SegmentBuffer, CalibrationBuffer, RegimeDetector, RegimeClassifier, RegimeManager, SegmentSummarizer, TitleBarParser, ContentTracker, WorkTypeClassifier
- `maekon-core`가 다음을 획득: TriggerParams/ResolvedParams, TriggerInput, CalibrationEntry, RegimeFeatures, Regime, SegmentSummary, ContentActivity, WorkType, EngagementMetrics, CalibrationWriter/CalibrationReader port, TieredMemoryConfig, PresetProfile
- SQLite V9 마이그레이션이 4개 테이블 추가: calibration_log, trigger_params_snapshots, regimes, activity_segments
- 외부 ML 의존성 없음 (k-means 수작업 구현)
- ContextAssembler가 `current_segment` + `current_regime` 파라미터 획득
- Monitor 루프가 기존 경로에 추가로 AdaptiveTrigger에 이벤트를 전달

## 참조

- ADR-011: 독립형 분석 파이프라인 (기반)
- ADR-001 §1-7: 오류 타입, async trait, DI, 크레이트 경계
- ADR-003: 디렉토리 모듈 패턴 (파일이 500줄을 초과하면 적용)
- 설계 사양: 내부 적응형 계층 메모리 설계 노트
- 연구: Dual-EWMA, ESPRESSO, CUSUM, MemGPT 메모리 통합
