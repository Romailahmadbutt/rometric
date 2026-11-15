//! Windows shell integration: tray menu and per-user autostart.

use rometric::branding;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};
use winreg::enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE};
use winreg::RegKey;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Rometric";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    ToggleEngine,
    ToggleBypass,
    ToggleAutostart,
    ApplyPreset(String),
    Quit,
}

pub struct TrayController {
    icon: TrayIcon,
    preset_names: Vec<String>,
}

impl TrayController {
    pub fn new(
        presets: &[String],
        engine_running: bool,
        correction_enabled: bool,
        autostart: bool,
    ) -> Result<Self, String> {
        let icon = TrayIconBuilder::new()
            .with_id("rometric")
            .with_tooltip(branding::APP_NAME)
            .with_menu_on_left_click(false)
            .with_icon(app_icon()?)
            .build()
            .map_err(|err| format!("Could not create tray icon: {err}"))?;
        let mut this = Self {
            icon,
            preset_names: Vec::new(),
        };
        this.refresh(presets, engine_running, correction_enabled, autostart)?;
        Ok(this)
    }

    pub fn refresh(
        &mut self,
        presets: &[String],
        engine_running: bool,
        correction_enabled: bool,
        autostart: bool,
    ) -> Result<(), String> {
        self.preset_names = presets.iter().take(8).cloned().collect();
        let menu = Menu::new();
        append(
            &menu,
            &MenuItem::with_id("show", "Show Rometric", true, None),
        )?;
        append(
            &menu,
            &MenuItem::with_id(
                "engine",
                if engine_running {
                    "Stop audio"
                } else {
                    "Start audio"
                },
                true,
                None,
            ),
        )?;
        append(
            &menu,
            &CheckMenuItem::with_id(
                "bypass",
                "Correction enabled",
                true,
                correction_enabled,
                None,
            ),
        )?;
        append(
            &menu,
            &CheckMenuItem::with_id("autostart", "Start with Windows", true, autostart, None),
        )?;
        if !self.preset_names.is_empty() {
            append(&menu, &PredefinedMenuItem::separator())?;
            for (index, name) in self.preset_names.iter().enumerate() {
                append(
                    &menu,
                    &MenuItem::with_id(
                        format!("preset:{index}"),
                        format!("Use preset: {name}"),
                        true,
                        None,
                    ),
                )?;
            }
        }
        append(&menu, &PredefinedMenuItem::separator())?;
        append(&menu, &MenuItem::with_id("quit", "Quit", true, None))?;
        self.icon.set_menu(Some(Box::new(menu)));
        Ok(())
    }

    pub fn set_tooltip(&self, status: &str) {
        let _ = self
            .icon
            .set_tooltip(Some(format!("{}\n{status}", branding::APP_NAME)));
    }

    pub fn poll(&self) -> Vec<TrayAction> {
        let mut actions = Vec::new();
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let id = event.id.0.as_str();
            let action = match id {
                "show" => Some(TrayAction::Show),
                "engine" => Some(TrayAction::ToggleEngine),
                "bypass" => Some(TrayAction::ToggleBypass),
                "autostart" => Some(TrayAction::ToggleAutostart),
                "quit" => Some(TrayAction::Quit),
                _ => id
                    .strip_prefix("preset:")
                    .and_then(|value| value.parse::<usize>().ok())
                    .and_then(|index| self.preset_names.get(index).cloned())
                    .map(TrayAction::ApplyPreset),
            };
            if let Some(action) = action {
                actions.push(action);
            }
        }
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                actions.push(TrayAction::Show);
            }
        }
        actions
    }
}

fn append(menu: &Menu, item: &dyn tray_icon::menu::IsMenuItem) -> Result<(), String> {
    menu.append(item).map_err(|err| err.to_string())
}

fn app_icon() -> Result<Icon, String> {
    let logo = branding::logo_rgba()?;
    Icon::from_rgba(logo.rgba, logo.width, logo.height).map_err(|err| err.to_string())
}

fn autostart_command() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    Ok(format!("\"{}\" --minimized", exe.display()))
}

pub fn autostart_enabled() -> bool {
    let Ok(key) =
        RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, KEY_QUERY_VALUE)
    else {
        return false;
    };
    matches!(
        key.get_value::<String, _>(RUN_VALUE),
        Ok(value) if value == autostart_command().unwrap_or_default()
    )
}

pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE | KEY_QUERY_VALUE)
        .map_err(|err| format!("Could not open Windows startup settings: {err}"))?;
    if enabled {
        key.set_value(RUN_VALUE, &autostart_command()?)
            .map_err(|err| format!("Could not enable startup: {err}"))
    } else {
        match key.delete_value(RUN_VALUE) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(format!("Could not disable startup: {err}")),
        }
    }
}
