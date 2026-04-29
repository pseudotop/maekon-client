#!/bin/bash
# 프론트엔드 빌드 스크립트
#
# 사용법: ./scripts/build-frontend.sh
#
# React 프론트엔드를 빌드하여 dist/ 폴더에 생성합니다.
# rust-embed가 이 파일들을 바이너리에 임베드합니다.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
FRONTEND_DIR="$PROJECT_ROOT/crates/oneshim-web/frontend"

echo "🔨 프론트엔드 빌드 시작..."
echo "   경로: $FRONTEND_DIR"

cd "$FRONTEND_DIR"

# pnpm 설치 확인
if ! command -v pnpm &> /dev/null; then
    echo "❌ pnpm이 설치되어 있지 않습니다."
    echo "   npm install -g pnpm 명령으로 설치하세요."
    exit 1
fi

# 의존성 설치
if [ ! -d "node_modules" ]; then
    echo "📦 의존성 설치 중..."
    pnpm install
fi

# 빌드
echo "⚙️  빌드 중..."
pnpm build

# 결과 확인
if [ -f "dist/index.html" ]; then
    echo ""
    echo "✅ 프론트엔드 빌드 완료!"
    echo "   출력: $FRONTEND_DIR/dist/"
    echo ""
    ls -lh dist/
    echo ""
    echo "   이제 cargo build로 Rust 바이너리를 빌드하면"
    echo "   프론트엔드가 바이너리에 임베드됩니다."
else
    echo "❌ 빌드 실패: dist/index.html이 생성되지 않았습니다."
    exit 1
fi
