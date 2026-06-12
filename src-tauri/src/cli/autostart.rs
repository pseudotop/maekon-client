use super::DebugAutostartCliCommand;

pub(crate) fn run_debug_autostart_cli_command(command: DebugAutostartCliCommand) -> i32 {
    match command {
        DebugAutostartCliCommand::Status => match crate::autostart::is_autostart_enabled() {
            Ok(enabled) => {
                let capabilities = crate::autostart::detect_capabilities();
                println!(
                    "{{\"debug_autostart\":true,\"command\":\"status\",\"enabled\":{},\"supported\":{},\"environment\":\"{:?}\"}}",
                    enabled, capabilities.supported, capabilities.environment
                );
                0
            }
            Err(error) => {
                eprintln!("debug-autostart status failed: {error}");
                1
            }
        },
        DebugAutostartCliCommand::Enable => match crate::autostart::enable_autostart() {
            Ok(()) => {
                println!("{{\"debug_autostart\":true,\"command\":\"enable\",\"ok\":true}}");
                0
            }
            Err(error) => {
                eprintln!("debug-autostart enable failed: {error}");
                1
            }
        },
        DebugAutostartCliCommand::Disable => match crate::autostart::disable_autostart() {
            Ok(()) => {
                println!("{{\"debug_autostart\":true,\"command\":\"disable\",\"ok\":true}}");
                0
            }
            Err(error) => {
                eprintln!("debug-autostart disable failed: {error}");
                1
            }
        },
    }
}
