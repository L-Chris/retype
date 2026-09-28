fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return Ok(());
    }
    let icon = "../../../apps/settings/windows/runner/resources/app_icon.ico";
    println!("cargo:rerun-if-changed={icon}");
    // The registered TSF profile points at this DLL. Its first icon resource is
    // the branding icon shown to the right of the 中/A mode indicator.
    let mut resources = winres::WindowsResource::new();
    resources
        .set_icon(icon)
        .set_version_info(winres::VersionInfo::FILETYPE, 2);
    resources.compile()?;
    Ok(())
}
