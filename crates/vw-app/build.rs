//! Windows では、エクスプローラやタスクバーに出す EXE のアイコンを埋め込む。

fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icons/vw.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../packaging/icons/vw.ico");
        res.compile().expect("embed the icon into vw.exe");
    }
}
