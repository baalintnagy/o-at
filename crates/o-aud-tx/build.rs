//! Embeds the O-AUD-TX icon and version information in Windows executables.

use std::env;

use winresource::WindowsResource;

const ICON: &str = "../../assets/icons/o-aud-tx.ico";

fn main() {
    println!("cargo:rerun-if-changed={ICON}");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = WindowsResource::new();
    resource
        .set_icon(ICON)
        .set("ProductName", "O-AUD-TX")
        .set("FileDescription", "O-AUD-TX audio bus transmitter")
        .set(
            "LegalCopyright",
            "Copyright (c) 2026 baalintnagy, MIT License",
        );
    resource
        .compile()
        .expect("the Windows resource compiler embeds the icon");
}
