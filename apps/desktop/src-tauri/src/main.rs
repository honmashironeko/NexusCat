#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    nc_platform::enable_dpi_awareness();
    #[cfg(windows)]
    if nc_desktop_lib::input_service::handle_cli_mode() {
        return;
    }
    #[cfg(windows)]
    if nc_desktop_lib::handle_file_drop_broker_cli_mode() {
        return;
    }
    if !nc_desktop_lib::elevation::ensure_elevated() {
        return;
    }
    nc_desktop_lib::run();
}
