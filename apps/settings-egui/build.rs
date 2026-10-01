fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winres::WindowsResource::new();
        resource.set_icon("../settings/windows/runner/resources/app_icon.ico");
        resource.set("FileDescription", "retype Settings");
        resource.set("ProductName", "retype");
        if let Err(error) = resource.compile() {
            eprintln!("Failed to compile settings resources: {error}");
            std::process::exit(1);
        }
    }
}
