use crate::Msg;
use serde_json::json;
use tao::event_loop::EventLoopProxy;
use webview2_com::{AcceleratorKeyPressedEventHandler, Microsoft::Web::WebView2::Win32::*};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT};
use wry::{WebView, WebViewExtWindows};

pub fn attach(view: &WebView, proxy: EventLoopProxy<Msg>) -> windows::core::Result<()> {
    unsafe {
        let mut token = 0;
        view.controller().add_AcceleratorKeyPressed(
            &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN;
                args.KeyEventKind(&mut kind)?;
                if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN {
                    return Ok(());
                }
                if GetKeyState(VK_CONTROL.0 as i32) >= 0 {
                    return Ok(());
                }
                let mut key = 0;
                args.VirtualKey(&mut key)?;
                let cmd = match key {
                    0x4e if GetKeyState(VK_SHIFT.0 as i32) < 0 => "stealth",
                    0x4c => "focus_address",
                    0x54 => "new",
                    0x57 => "close_current",
                    0x44 => "bookmark",
                    0x48 => "history",
                    0x4a => "downloads",
                    0x52 => "reload",
                    0x09 if GetKeyState(VK_SHIFT.0 as i32) < 0 => "previous_tab",
                    0x09 => "next_tab",
                    _ => return Ok(()),
                };
                args.SetHandled(true)?;
                let _ = proxy.send_event(Msg::Ui(json!({"cmd":cmd})));
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}
