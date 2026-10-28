//! Windows では、エクスプローラやタスクバーに出す EXE のアイコンと、プロパティの「詳細」に出す
//! 製品名・説明・版・著作権を埋め込む。

fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icons/vw.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../packaging/icons/vw.ico");
        res.set("ProductName", "vw Image Viewer");
        res.set("FileDescription", "vw Image Viewer");
        // about.rs の COPYRIGHT と同じ文言。build.rs からはクレートのモジュールを読めないので写す。
        res.set("LegalCopyright", "Copyright (c) 2026 vw contributors");
        res.compile()
            .expect("embed the icon and version info into vw.exe");
    }
}
