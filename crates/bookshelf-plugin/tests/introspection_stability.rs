#[test]
fn introspection_is_stable_under_epoch_pump() {
    for file in [
        "../../plugins-built/hello.wasm",
        "../../plugins-built/wiki.wasm",
        "../../plugins-built/reader.wasm",
    ] {
        let wasm = std::fs::read(file).expect("read");
        let plugin = bookshelf_plugin::WasmPlugin::load("t.wasm", wasm).expect("load");
        let mut fails = 0;
        let mut last = String::new();
        for i in 0..30 {
            let r = plugin.name();
            if r.is_err() {
                fails += 1;
                last = format!("{r:?}");
            }
            let c = plugin.capabilities();
            if c.is_err() && fails < 1 {
                fails += 1;
                last = format!("{c:?}");
            }
            let s = plugin.config_schema();
            if s.is_err() && fails < 1 {
                fails += 1;
                last = format!("{s:?}");
            }
        }
        eprintln!("{file}: {fails} failures, last={last}");
        assert!(fails == 0, "{file} failed {fails} times, last={last}");
    }
}
