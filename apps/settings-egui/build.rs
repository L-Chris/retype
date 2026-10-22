fn main() {
    let icon = "../../assets/retype.ico";
    println!("cargo:rerun-if-changed={icon}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winres::WindowsResource::new();
        resource.set_icon(icon);
        resource.set("FileDescription", "retype Settings");
        resource.set("ProductName", "retype");
        if let Err(error) = resource.compile() {
            eprintln!("Failed to compile settings resources: {error}");
            std::process::exit(1);
        }
    }
}
