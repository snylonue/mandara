# Bookshelf · 轻小说阅读网站

自托管的轻小说阅读网站：Rust 后端 + Web 前端，支持 **epub / txt**，多用户，
书籍与元数据统一存储，可选的权限控制，以及通过 **wasm 插件** 提供自定义的
书籍/元数据来源。

## 功能特性

- 📖 支持 epub（HTML→纯文本转换）与 txt（UTF-8 / UTF-16 / GB18030 编码探测，
  中文/英文章节标题切分）上传与阅读
- 👥 多用户：注册 / 登录（JWT + Argon2），`admin` / `user` 角色
- 🔒 可选权限控制：`BOOKSHELF_AUTH_ENABLED=false` 即单用户本地模式，
  一切操作以本地 admin 身份进行；书籍 `private`（仅所有者+管理员）/ `public` 可见性
- 🗄️ 统一存储：所有书籍、元数据、章节内容都在同一个 SQLite 数据库中
  （`data/bookshelf.db`），备份 = 拷一个文件
- 🧩 wasm 插件系统：`data/plugins/*.wasm` 在启动时加载为 WebAssembly 组件
  （wasmtime + component model），插件实现 `bookshelf:plugin` world 即可提供
  自己的书目/元数据来源；章节在首次阅读时按需物化进中央库
- 📑 多会话进度：同一本书可有多个阅读会话（手机/平板/电脑各一个），
  各自独立记录进度（章节 + 章节内滚动比例）
- 🔗 分享：书籍分享链接（无需登录即可阅读）；会话进度分享链接
  （跟随某次阅读的进度，支持多设备同步/朋友互相追进度）

## 技术栈

| 层 | 技术 |
|---|---|
| 后端 | Rust (edition 2021)，axum 0.8，sqlx 0.9 (SQLite)，jsonwebtoken，argon2 |
| 插件宿主 | wasmtime 48 (component model)，WIT 接口定义在 `crates/bookshelf-plugin/wit/` |
| 前端 | Vite 8 + React 19 + TypeScript (npm 管理依赖) |
| 环境 | Nix (flake-parts + rust-overlay)，临时工具一律 `nix run` |

## 目录结构

```
├── flake.nix                    # nix 开发环境（rust 工具链+wasm 目标、node、wasm-tools）
├── Cargo.toml                   # cargo workspace
├── crates/
│   ├── bookshelf-core/          # 领域模型 + BookSource trait（插件接缝）
│   ├── bookshelf-formats/       # epub / txt 解析
│   ├── bookshelf-plugin/        # wasmtime 组件宿主 + WIT 接口
│   └── bookshelf-server/        # axum 服务：路由、认证、库、会话、分享、migrations/
├── plugins/
│   └── hello-plugin/            # 示例插件（构建为 wasm 组件）
├── frontend/                    # React 前端（npm）
├── docs/plugins.md              # 插件开发指南
└── justfile                     # 常用命令
```

## 快速开始

```sh
# 1. 进入开发环境（rust 1.98 + node 24 + wasm-tools，首次会下载工具链）
nix develop

# 2. （可选）构建并安装示例 wasm 插件
just plugin-build        # 生成 plugins-built/hello.wasm
cp plugins-built/hello.wasm data/plugins/

# 3. 启动后端（默认 http://127.0.0.1:8080，数据在 data/）
just dev

# 4. 另开终端启动前端（http://localhost:5173，/api 代理到后端）
just dev-web
```

生产/单二进制部署：构建前端产物后由后端直接托管：

```sh
cd frontend && npm install && npm run build   # 生成 frontend/dist
cargo run -p bookshelf-server                 # 自动托管 frontend/dist (SPA)
```

> 提示：本机 `~/.npm` 缓存若被 root 占用导致 `npm install` 报 EACCES，
> 可临时用 `npm_config_cache=/tmp/npm-cache` 绕过。

## 配置（环境变量 / CLI 参数）

| 变量 | 默认值 | 说明 |
|---|---|---|
| `BOOKSHELF_ADDR` | `127.0.0.1:8080` | 监听地址 |
| `BOOKSHELF_DB` | `data/bookshelf.db` | SQLite 数据库（统一存储） |
| `BOOKSHELF_DATA_DIR` | db 所在目录 | 运行数据目录 |
| `BOOKSHELF_PLUGINS_DIR` | `data/plugins` | 扫描 `*.wasm` 插件 |
| `BOOKSHELF_JWT_SECRET` | `dev-only-change-me` | JWT 密钥（生产必须改） |
| `BOOKSHELF_AUTH_ENABLED` | `true` | 设为 `false` 关闭认证/权限（单用户模式） |
| `BOOKSHELF_ALLOW_REGISTER` | `true` | 允许注册新用户 |
| `BOOKSHELF_MAX_UPLOAD_MB` | `64` | 上传大小上限 |
| `BOOKSHELF_FRONTEND_DIR` | `frontend` | 前端目录（其 `dist/` 存在时托管于 `/`） |

复制 `.env.example` 为 `.env` 可覆盖后四项以外的默认值。

## 数据模型与进度/分享语义

```
users    ─┬─< books   (owner_id, visibility)
          │
          ├─< sessions (user_id, book_id, label, chapter_idx, offset, fraction)
          │
          └─< shares   (token, kind=book|session, mode=read|progress,
                        book_id, session_id, expires_at)
chapters (book_id, idx, title, content)   # 统一内容存储
```

- **位置 (Position)** = 章节索引 + 章节内字符偏移 + 章节内滚动比例 (0..1)。
  进度接口接受三者，自动按书本章节数裁剪（`Position::clamped`）。
- **会话 (Session)**：同一用户对同一本书可建多个命名会话（唯一约束
  `(user_id, book_id, label)`，重名自动复用）。
- **分享**：`book` 分享给未登录访客只读权限；`session` 分享展示该会话的
  实时进度（方位百分比 + 章节 + 更新时间），拿链接的人可以跟着读同一进度。

## API 概览

```
POST /api/auth/register|login            → {token, user}
GET  /api/auth/me
GET  /api/health                         → {auth_enabled, allow_register, ...}

GET  /api/books?q=&source=               → 可见书目列表
POST /api/books                          ← multipart file (epub/txt)
GET  /api/books/{id}                     → {book, chapters:[{idx,title}]}
PATCH/DELETE /api/books/{id}             （所有者/管理员）
GET  /api/books/{id}/chapters/{idx}      → 章节内容（插件章节按需物化）

GET/POST /api/books/{id}/sessions        → 我的会话列表 / 新建会话 {label}
PUT/DELETE /api/sessions/{id}            → 更新进度 {chapter_idx,offset,fraction} / 删除

POST /api/books/{id}/shares              → {kind:"book"|"session", session_id?, expires_days?}
GET  /api/shares/{token}                 → 分享信息（会话快照）
GET  /api/shares/{token}/book            → 匿名读：书目+章节目录
GET  /api/shares/{token}/chapters/{idx}  → 匿名读：章节
DELETE /api/shares/{token}               （创建者/管理员）

GET  /api/plugins                        → 已加载插件
POST /api/plugins/sync                   （管理员）重新同步插件书目
```

## wasm 插件

编写一个实现 `bookshelf:plugin` world 的组件，放入 `data/plugins/` 即被加载。
完整指南见 **[docs/plugins.md](docs/plugins.md)**；仓库内置示例 `plugins/hello-plugin`：

```sh
just plugin-build          # cargo build --target wasm32-unknown-unknown
                           # + wasm-tools component new
```

## 开发命令

```sh
just dev          # 启动后端
just dev-web      # 启动前端 dev server
just check        # cargo check + clippy
just test         # cargo test --workspace
just plugin-build # 构建示例插件组件
```

## License

MIT OR Apache-2.0