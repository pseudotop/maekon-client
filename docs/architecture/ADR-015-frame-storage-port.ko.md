[English](./ADR-015-frame-storage-port.md) | [한국어](./ADR-015-frame-storage-port.ko.md)

# ADR-015: 프레임 스토리지 포트 추상화

**상태**: Accepted
**날짜**: 2026-04-03
**범위**: `oneshim-core` 포트, `oneshim-storage` 어댑터, `src-tauri` 컴포지션 루트

---

## 컨텍스트

`FrameFileStorage`는 `oneshim-storage`의 구체 타입으로, 프레임 이미지 저장
(날짜별 디렉터리 + 보존 정책)을 담당한다.

현재 `src-tauri`의 10개 이상 파일에서 `Arc<FrameFileStorage>`를 직접 참조하고 있어,
다른 스토리지 작업이 따르는 헥사고날 포트 추상화를 우회하고 있다.

## 결정

`oneshim-core::ports`에 `FrameStoragePort` trait를 도입하여 소비자가 실제로
사용하는 프레임 스토리지 작업을 추상화한다:

- `save_frame` — 단일 프레임 저장
- `save_frames_batch` — 배치 프레임 저장
- `enforce_retention` — 보존 기간 초과 프레임 삭제
- `enforce_storage_limit` — 스토리지 용량 제한 적용

진단 메서드(`frames_dir`, `buffer_pool_stats`, `disk_status`)는 구체 타입에 유지.

## 효과

- 프레임 스토리지 소비자가 mock으로 테스트 가능
- 향후 스토리지 백엔드(인메모리, 클라우드) 교체 시 소비자 변경 불필요
- 의존성 그래프 명확화 — `oneshim-storage`는 와이어링 코드에서만 참조
