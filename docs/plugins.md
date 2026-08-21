# 插件开发指南

Bookshelf 的 wasm 插件系统让用户可以自定义书籍与元数据来源：插件以
**WebAssembly 组件** 的形式在宿主（wasmtime）沙箱中运行，只能调用
`store::log`，无法访问宿主文件系统或网络（如需访问网络，后续可扩展 WIT
增加宿主提供的接口）。

插件开机自动加载（`data/plugins/*.wasm`），加载后：

1. 启动时调用 `list-books` + `chapter-titles` 把书目与章节标题**同步进中央库**
   （统一元数据存储）；
2. 读者首次请求某章节内容时按需调用 `get-chapter`，结果**物化**进中央库，
   之后与本地书无异（删除插件后内容仍在库里）。
3. 管理员可随时 `POST /api/plugins/sync` 重新同步。

## 接口定义（WIT）

接口文件：`crates/bookshelf-plugin/wit/bookshelf.wit`，world `bookshelf:plugin/bookshelf-plugin`：

```wit
interface types {
    record book-entry {
        id: string,                // 插件内稳定 id
        title: string,
        authors: list<string>,
        description: option<string>,
        cover-url: option<string>,
    }
    record chapter { title: string, content: string }  // content 为纯文本
}

interface store {
    log: func(message: string);    // 宿主提供的日志
}

world bookshelf-plugin {
    import types;
    import store;
    use types.{book-entry, chapter};

    export name: func() -> string;
    export list-books: func() -> list<book-entry>;
    export chapter-titles: func(book-id: string) -> list<string>;
    export get-chapter: func(book-id: string, index: u32) -> option<chapter>;
}
```

## 编写插件

参考仓库内示例 `plugins/hello-plugin/`，骨架如下：

```toml
# Cargo.toml
[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = "0.60"
```

```rust
// src/lib.rs
wit_bindgen::generate!({
    world: "bookshelf-plugin",
    path: "../../crates/bookshelf-plugin/wit",
});

struct MyPlugin;

impl Guest for MyPlugin {
    fn name() -> String { "my-source".into() }
    fn list_books() -> Vec<BookEntry> { /* 返回书目 */ }
    fn chapter_titles(book_id: String) -> Vec<String> { /* 返回标题 */ }
    fn get_chapter(book_id: String, index: u32) -> Option<Chapter> { /* 返回内容 */ }
}

export!(MyPlugin);
```

> `Guest`、`BookEntry`、`Chapter`、`export!` 均由 `generate!` 宏在调用处生成。

## 构建（nix 环境）

```sh
nix develop                          # devShell 已含 wasm 目标与 wasm-tools

cargo build -p hello-plugin --release --target wasm32-unknown-unknown
wasm-tools component new \
    target/wasm32-unknown-unknown/release/hello_plugin.wasm \
    -o my-plugin.wasm

# 部署：复制到插件目录，重启服务（或用其它方式热替换后 POST /api/plugins/sync）
cp my-plugin.wasm data/plugins/
```

要点：

- 示例用 `wasm32-unknown-unknown` 目标构建纯计算组件：world 不 import 任何
  wasi 接口，`wit-bindgen` 生成的核心模块自带 `component-type` 自定义段，
  `wasm-tools component new` 无需 adapter 即可提升为组件。
- 插件内避免使用 `std::fs`/`std::net`/`println!`（unknown-unknown 目标
  下不可用或产生 wasi 导入导致 component 需要 adapter）。
- devShell 的 rust 工具链也安装了 `wasm32-wasip1` / `wasm32-wasip2` 目标；
  若插件确实需要 wasi（例如网络），可换用 wasip2 目标 + `wasm-tools component
  new` + 宿主侧引入 `wasmtime-wasi` 的 p2 linker（未来版本会补充示例）。
- 审计建议：插件是任意代码，请只加载可信来源的 `.wasm` 文件
  （wasmtime 提供内存安全沙箱，但不阻止插件"消耗"宿主的计算资源）。

## 调试

- 插件内调用 `log("...")`，宿主以 `INFO plugin: ...` 打印到服务日志；
- 加载失败的插件会被跳过并输出 `WARN skipping plugin: ...`（不会阻止服务启动）；
- 用 `wasm-tools component wit my-plugin.wasm` 查看组件已实现的接口。