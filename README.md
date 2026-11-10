# 📊 Rust SQLite Editor

엑셀처럼 데이터를 자유롭고 직관적으로 편집할 수 있는 가벼운 데스크톱 SQLite 에디터입니다.

복잡한 DB 관리 기능 대신, 로컬 `.db` 파일을 열어 스프레드시트처럼 셀과 행을 편집하고 안전하게 트랜잭션으로 반영하는 기능에 집중합니다.

---

## 🛠️ 주요 기능

- **엑셀 연동 및 복사·붙여넣기**:
  - `Ctrl+V`: 기존 셀 영역에 클립보드 데이터 덮어쓰기
  - `Ctrl+Shift+V`: 클립보드 데이터를 신규 행으로 일괄 추가
  - `Ctrl+C`: 전체 또는 선택 영역을 엑셀과 호환되는 TSV로 복사
- **정밀한 수정 내역 추적 (Dirty Tracking)**:
  - 기준점(Baseline) 대비 수정된 셀 `(Row, Col) -> (OldVal, NewVal)` 정밀 추적
  - 수정된 셀 시각적 표시 (앰버 배경 + 우측 상단 오렌지 코너 삼각형 + 원본값 툴팁)
  - 하단 실시간 변경 로그 및 원클릭 전체 되돌리기(Revert)
- **SQLite DB 연결 및 스키마 분석**:
  - 시스템 테이블 제외한 사용자 테이블 목록 및 레코드 수 실시간 집계
  - `PRAGMA table_info`를 통한 컬럼명, 타입 친화도, NOT NULL, 기본키(PK 🔑) 자동 추출 및 표시
  - 미저장 변경사항 보호 (테이블 전환 시 유실 방지 경고 팝업)
- **대용량 데이터 고속 처리**:
  - 1,000행 일괄 파싱 및 추가 약 10ms(0.01초) 이내 처리

---

## 🏗️ 기술 스택

| 구분 | 기술 |
| :--- | :--- |
| **언어** | Rust (Edition 2024) |
| **GUI 프레임워크** | `eframe` / `egui` (0.36) |
| **데이터 그리드** | `egui_tabular` (0.2) |
| **데이터베이스** | `rusqlite` (0.38, bundled SQLite) |
| **클립보드** | `arboard` |
| **네이티브 다이얼로그** | `rfd` |

---

## 🚀 실행 및 빌드

### 로컬 실행
```bash
cargo run
```

### 테스트 실행
```bash
cargo test
```

### 릴리즈 빌드
```bash
# Windows
cargo build --release

# macOS / Linux
cargo build --release
```

---

## 📦 GitHub Actions 자동 빌드

저장소의 GitHub Actions 워크플로우를 통해 코드가 푸시될 때마다 Windows 독립 실행 파일(`sqlite-editor.exe`) 및 macOS 바이너리가 자동으로 빌드되어 아티팩트로 다운로드할 수 있습니다.
