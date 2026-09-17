// Windows 的发布构建不要弹控制台窗口。
//
// `decorations:false` + `skipTaskbar:true` 已经让窗口没有任何系统外壳了，
// 再挂一个黑框子在任务栏里就白做了（§4.3 补充说明）。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    qq_drawer_lib::run();
}
