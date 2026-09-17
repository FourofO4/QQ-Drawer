//! 构建脚本。
//!
//! 除了正常的 `tauri-build`，这里还接手了 Windows 应用清单的生成 ——
//! 详见 `embed_windows_manifest` 的注释。

fn main() {
    // 关掉 tauri-build 自带的清单，改由本脚本统一发（原因见下）。
    // 图标、版本信息仍然由 tauri-build 提供，只是不再带 manifest。
    let attrs = tauri_build::Attributes::new()
        .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    tauri_build::try_build(attrs).expect("tauri-build 失败");

    embed_windows_manifest();
}

/// 给**所有**目标（尤其是 `cargo test --lib` 的测试宿主）嵌入 Windows 应用清单，
/// 声明对 Common-Controls v6 的依赖。
///
/// 为什么不能直接用 tauri-build 自带的那份：
///
/// 1. tauri-build（`tauri-winres` → `embed-resource`）发的是
///    `cargo:rustc-link-arg-bins=`，**只有 `[[bin]]` 拿得到**。测试宿主不生成清单，
///    加载器就把 `comctl32.dll` 绑到 System32 里的 legacy 5.82；而
///    `tauri-runtime-wry` 的 Windows 对话框代码硬引用了只有 v6 才导出的
///    `TaskDialogIndirect`。测试进程连 main 都进不去，直接以
///    `STATUS_ENTRYPOINT_NOT_FOUND (0xC0000139)` 退出，`cargo test` 只留一句
///    "process didn't exit successfully"，完全看不出原因。
///
/// 2. 换成更窄的 `cargo:rustc-link-arg-tests=` 也不行：cargo 的
///    `LinkArgTarget::Test` 判的是 `target.is_test()`，而 `tests/` 目录下的集成
///    测试才算数；包内单元测试（`--lib`）拿不到。没有 `tests/` 目录时 cargo 还会
///    直接报 "The package ... does not have a test target" 而中止构建。
///    （上游未修的 cargo#10937。）
///
/// 3. 所以只能用不限定的 `cargo:rustc-link-arg=`：cargo 里对应
///    `LinkArgTarget::All`，判断是 `=> true`，对所有目标无条件生效。
///    代价是它同样会作用于 `[[bin]]`，会和 tauri-build 那份重复；因此
///    `main()` 里先 `new_without_app_manifest()` 把自带那份关掉，保证每个二进制
///    里只有一份清单。
fn embed_windows_manifest() {
    // 用 CARGO_CFG_TARGET_OS 而不是 cfg!(windows)：交叉编译时两者会不一致。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // 与 tauri-build/src/windows-app-manifest.xml 内容一致：只声明 Common-Controls
    // v6 依赖。测试宿主不需要图标/版本信息，那些仍由 tauri-build 提供。
    const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#;

    let out_dir =
        std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR 由 cargo 提供"));
    let rc = out_dir.join("qq-drawer-manifest.rc");

    // .rc 里 name/type 写作 `1 24`（CREATEPROCESS_MANIFEST_RESOURCE_ID / RT_MANIFEST）。
    // 多行清单要写成一串带引号的字符串，字面量 `"` 需双写 —— 与
    // tauri-winres::write_resource_file 的写法一致。
    let mut src = String::from("#pragma code_page(65001)\n1 24\n{\n");
    for line in MANIFEST.lines() {
        src.push_str("\" ");
        src.push_str(&line.replace('"', "\"\""));
        src.push_str(" \"\n");
    }
    src.push_str("}\n");
    std::fs::write(&rc, src).expect("写入清单 .rc 失败");

    embed_resource::compile_for_everything(&rc, embed_resource::NONE)
        .manifest_required()
        .expect("编译 Windows 资源失败（GNU 工具链需要 PATH 里有 windres 和 ar）");
}
