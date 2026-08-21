set shell := ["bash", "-uc"]

# 启动后端开发服务器（默认 http://127.0.0.1:8080，数据在 data/）
dev:
    cargo run -p bookshelf-server

# 启动前端开发服务器（http://localhost:5173，/api 代理到后端）
dev-web:
    cd frontend && npm run dev

# 检查 + 测试
check:
    cargo check --workspace
    cargo clippy --workspace -- -D warnings

test:
    cargo test --workspace

fmt:
    cargo fmt --all

# 构建示例 wasm 插件 → plugins-built/hello.wasm
# 部署：cp plugins-built/hello.wasm data/plugins/
plugin-build:
    ./scripts/build-plugin-hello.sh

# 构建前端产物 frontend/dist（后端会直接托管它）
web-build:
    cd frontend && npm install && npm run build