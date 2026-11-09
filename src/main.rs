#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod background;
mod blocker;
mod capture;
mod downloads;
mod identity;
mod migration;
mod shortcuts;
mod storage;
mod suggestions;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    fs,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};
use storage::*;
use tao::{
    dpi::{LogicalPosition, LogicalSize},
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    platform::windows::EventLoopBuilderExtWindows,
    window::{Window, WindowBuilder},
};
use webview2_com::{
    take_pwstr, BrowserExtensionRemoveCompletedHandler, Microsoft::Web::WebView2::Win32::*,
    NewWindowRequestedEventHandler, ProfileAddBrowserExtensionCompletedHandler,
    ProfileGetBrowserExtensionsCompletedHandler, SourceChangedEventHandler,
    TrySuspendCompletedHandler,
};
use windows::core::{Interface, HSTRING, PWSTR};
use wry::{
    NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder,
    WebViewBuilderExtWindows, WebViewExtWindows,
};
const TOP: f64 = 108.0;
const VISIBLE_FIELD_JS: &str = r#"e=>{if(!e||e.hidden||e.closest('[hidden],[inert]')||!e.getClientRects().length||e.disabled||e.readOnly)return false;const s=getComputedStyle(e);return s.display!=='none'&&s.visibility!=='hidden'&&s.visibility!=='collapse'}"#;
#[derive(Debug)]
enum Msg {
    CloseWindow,
    Reopen,
    ShowDownloads,
    Exit,
    RestoreTray,
    Ui(Value),
    Navigate(usize, String),
    Title(usize, String),
    Loaded(usize, String),
    Popup(String, bool),
    RefreshUrl(usize),
    DownloadPrompt(usize),
    DownloadChanged,
    Toast(String),
    Filters(Result<String, String>),
    Tick,
    Extensions(Vec<Value>),
}
struct Tab {
    id: usize,
    url: String,
    title: String,
    view: Option<WebView>,
    source: Rc<RefCell<String>>,
    blocked: Rc<Cell<usize>>,
    downloading: Rc<Cell<usize>>,
    used: Instant,
    private: bool,
    suspend_requested: Cell<bool>,
}
struct Browser {
    window: Window,
    shell: WebView,
    regular_keeper: Option<WebView>,
    stealth_keeper: Option<WebView>,
    stealth_session: usize,
    tabs: Vec<Tab>,
    active: usize,
    next: usize,
    panel: bool,
    suggestions_open: bool,
    suggestions_height: f64,
    login_sites: RefCell<Option<Vec<String>>>,
    root: PathBuf,
    settings: Settings,
    shield: Rc<RefCell<blocker::Shield>>,
    proxy: EventLoopProxy<Msg>,
    downloads: downloads::Shared,
    updating: bool,
    settings_dirty: bool,
    settings_writable: bool,
    library_dirty: Cell<bool>,
    background: bool,
    tray: background::Tray,
    tray_tip: Option<String>,
    _activation: background::Instance,
    pending_download_prompts: Vec<usize>,
    download_error: Option<String>,
    exiting: bool,
}
impl Browser {
    fn persist_downloads(&mut self, force: bool) -> bool {
        match downloads::persist(&self.downloads, &self.root, force) {
            Ok(()) => {
                self.download_error = None;
                true
            }
            Err(error) => {
                if self.download_error.as_ref() != Some(&error) {
                    self.toast(&format!("Download history could not be saved: {error}"));
                }
                self.download_error = Some(error);
                false
            }
        }
    }
    fn update_tray(&mut self) -> bool {
        if self.background
            || downloads::needs_background(
                &self.downloads,
                self.settings.background_private_downloads,
            )
        {
            let tip = if self.background {
                "Yas — background downloads"
            } else {
                "Yas — download manager"
            };
            if self.tray_tip.as_deref() != Some(tip) {
                if let Err(error) = self.tray.show(tip) {
                    self.toast(&error);
                    return false;
                }
                self.tray_tip = Some(tip.into());
            }
        } else if self.tray_tip.take().is_some() {
            self.tray.hide();
        }
        true
    }
    fn close_private_tabs(&mut self, keep_downloading: bool) {
        let ids: Vec<_> = self
            .tabs
            .iter()
            .filter(|tab| tab.private && (!keep_downloading || tab.downloading.get() == 0))
            .map(|tab| tab.id)
            .collect();
        for id in ids {
            self.ui(json!({"cmd":"close","id":id}));
        }
    }
    fn shutdown(&mut self) {
        self.exiting = true;
        self.close_private_tabs(false);
        downloads::cancel_all(&self.downloads);
        self.persist_downloads(true);
        self.save_now();
        self.tray.hide();
        self.tray_tip = None;
    }
    // X closes the window; native transfer owners stay on this event-loop
    // thread. Explicit Exit is the separate, destructive stop action.
    fn close_window(&mut self) -> bool {
        self.close_private_tabs(self.settings.background_private_downloads);
        self.save_now();
        self.persist_downloads(true);
        if !downloads::needs_background(&self.downloads, self.settings.background_private_downloads)
        {
            self.shutdown();
            return true;
        }
        if !self.update_tray() {
            return false;
        }
        let _ = self.shell.evaluate_script("render({background:true})");
        self.background = true;
        self.window.set_visible(false);
        for tab in &mut self.tabs {
            if tab.downloading.get() == 0 {
                tab.view.take();
                continue;
            }
            if let Some(view) = &tab.view {
                let _ = view.set_visible(false);
                // Keep download producers running, including script-fed blob
                // streams. Other website views are released above.
            }
        }
        let _ = self.shell.set_visible(false);
        unsafe {
            if let Ok(web) = self.shell.webview().cast::<ICoreWebView2_3>() {
                let _ =
                    web.TrySuspend(&TrySuspendCompletedHandler::create(Box::new(|_, _| Ok(()))));
            }
        }
        self.update_tray();
        let _ = self.tray.balloon(
            "Yas is continuing your downloads",
            "Reopen Yas or use this tray icon to manage them. Exit and stop downloads ends unfinished transfers.",
        );
        false
    }
    fn show_window(&mut self, downloads: bool) {
        let was_background = self.background;
        self.background = false;
        self.window.set_visible(true);
        self.window.set_minimized(false);
        unsafe {
            if let Ok(web) = self.shell.webview().cast::<ICoreWebView2_3>() {
                let _ = web.Resume();
            }
        }
        let _ = self.shell.set_visible(true);
        self.snapshot();
        if downloads || was_background {
            self.panel = true;
            self.suggestions_open = false;
            let _ = self.shell.evaluate_script("showPanel('downloads')");
        }
        self.layout();
        self.window.set_focus();
        self.focus_active();
        self.update_tray();
        self._activation.acknowledge();
        for id in downloads::pending_ids(&self.downloads) {
            if !self.pending_download_prompts.contains(&id) {
                self.pending_download_prompts.push(id);
            }
        }
        for id in self.pending_download_prompts.drain(..) {
            let _ = self.proxy.send_event(Msg::DownloadPrompt(id));
        }
        if let Some(error) = &self.download_error {
            self.toast(&format!("Download history needs attention: {error}"));
        }
    }
    fn background_finished(&mut self) -> bool {
        if !self.background {
            return false;
        }
        if !downloads::has_private_jobs(&self.downloads) {
            self.close_private_tabs(false);
        }
        if !downloads::needs_background(&self.downloads, self.settings.background_private_downloads)
            && self.persist_downloads(true)
        {
            self.shutdown();
            return true;
        }
        false
    }
    fn release_background_views(&mut self) {
        if self.background {
            for tab in &mut self.tabs {
                if tab.downloading.get() == 0 {
                    tab.view.take();
                }
            }
        }
    }
    fn bounds(&self) -> Rect {
        let s = self
            .window
            .inner_size()
            .to_logical::<f64>(self.window.scale_factor());
        Rect {
            position: LogicalPosition::new(0., TOP).into(),
            size: LogicalSize::new(s.width, (s.height - TOP).max(1.)).into(),
        }
    }
    fn active_tab(&self) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == self.active)
    }
    fn focus_active(&self) {
        if self.background {
            return;
        }
        if self.panel
            || self.suggestions_open
            || self.active_tab().is_none_or(|t| t.url == "yas://newtab")
        {
            let _ = self.shell.focus();
        } else if let Some(view) = self.active_tab().and_then(|t| t.view.as_ref()) {
            let _ = view.focus();
        }
    }
    fn toast(&self, s: &str) {
        if self.background {
            let _ = self.tray.balloon("Yas downloads", s);
            return;
        }
        let _ = self.shell.evaluate_script(&format!("toast({})", json!(s)));
    }
    fn snapshot(&self) {
        if self.background {
            return;
        }
        let tabs:Vec<_>=self.tabs.iter().map(|t|json!({"id":t.id,"url":t.url,"title":t.title,"sleeping":t.view.is_none()&&t.url!="yas://newtab","private":t.private,"blocked":t.blocked.get()})).collect();
        let host = self
            .active_tab()
            .and_then(|t| url::Url::parse(&t.url).ok())
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default();
        let shield = self.shield.borrow();
        let mut data = json!({"windowMaximized":self.window.is_maximized(),"tabs":tabs,"active":self.active,"downloads":self.downloads.borrow().records,"blocked":shield.count,"rules":shield.rules,"shield":!shield.exceptions.contains(&host),"custom":self.settings.custom_filters,"updating":self.updating,"backgroundPrivateDownloads":self.settings.background_private_downloads,"background":false});
        if self.library_dirty.replace(false) {
            data["bookmarks"] = json!(self.settings.bookmarks);
            data["history"] = json!(self
                .settings
                .history
                .iter()
                .rev()
                .take(300)
                .collect::<Vec<_>>());
            data["imported_tabs"] = json!(self.settings.imported_tabs);
            data["reading_list"] = json!(self.settings.reading_list);
        }
        let _ = self.shell.evaluate_script(&format!("render({data})"));
    }
    fn save(&mut self) {
        self.settings.session = self
            .tabs
            .iter()
            .filter(|t| !t.private)
            .map(|t| t.url.clone())
            .collect();
        self.settings_dirty = true;
    }
    // Coalesce navigation/title/tab events; flush at most once per tick and on exit.
    fn save_now(&mut self) {
        self.save();
        if !self.settings_writable {
            self.settings_dirty = false;
            return;
        }
        if let Err(e) = write_settings(&self.root, &self.settings) {
            self.toast(&format!("Could not save settings: {e}"));
        } else {
            self.settings_dirty = false;
        }
    }
    fn layout(&self) {
        if self.background {
            let _ = self.shell.set_visible(false);
            for tab in &self.tabs {
                if let Some(view) = &tab.view {
                    let _ = view.set_visible(false);
                }
            }
            return;
        }
        let full = self.panel || self.active_tab().is_none_or(|t| t.url == "yas://newtab");
        let s = self
            .window
            .inner_size()
            .to_logical::<f64>(self.window.scale_factor());
        let _ = self.shell.set_bounds(Rect {
            position: LogicalPosition::new(0., 0.).into(),
            size: LogicalSize::new(
                s.width,
                if full {
                    s.height
                } else if self.suggestions_open {
                    self.suggestions_height.min(s.height)
                } else {
                    TOP
                },
            )
            .into(),
        });
        for t in &self.tabs {
            if let Some(v) = &t.view {
                let visible = t.id == self.active && !full;
                let _ = v.set_visible(visible);
                let _ = v.set_bounds(self.bounds());
                unsafe {
                    if let Ok(web) = v.webview().cast::<ICoreWebView2_3>() {
                        if visible || t.downloading.get() > 0 {
                            if t.suspend_requested.replace(false) {
                                let _ = web.Resume();
                            }
                        } else if t.id != self.active && !t.suspend_requested.replace(true) {
                            // Best effort: WebView2 protects media/download activity itself.
                            let _ = web.TrySuspend(&TrySuspendCompletedHandler::create(Box::new(
                                |_, _| Ok(()),
                            )));
                        }
                    }
                }
            }
        }
        if self.suggestions_open {
            use windows::Win32::{
                Foundation::HWND,
                UI::WindowsAndMessaging::{
                    SetWindowPos, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
                },
            };
            unsafe {
                let mut parent = HWND::default();
                if self.shell.controller().ParentWindow(&mut parent).is_ok() {
                    let _ = SetWindowPos(
                        parent,
                        Some(HWND_TOP),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                    );
                }
            }
        }
    }
    fn add(&mut self, u: &str, private: bool) {
        let id = self.next;
        self.next += 1;
        self.tabs.push(Tab {
            id,
            url: destination(u),
            title: "New tab".into(),
            view: None,
            source: Rc::new(RefCell::new(String::new())),
            blocked: Rc::new(Cell::new(0)),
            downloading: Rc::new(Cell::new(0)),
            used: Instant::now(),
            private,
            suspend_requested: Cell::new(false),
        });
        self.switch(id);
        self.save();
    }
    fn switch(&mut self, id: usize) {
        if !self.tabs.iter().any(|t| t.id == id) {
            return;
        }
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == self.active) {
            t.used = Instant::now();
        }
        self.active = id;
        self.panel = false;
        self.suggestions_open = false;
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
            t.used = Instant::now();
        }
        if !self.background && !self.exiting {
            if let Err(e) = self.ensure_view() {
                self.toast(&e)
            }
        }
        self.enforce_memory_budget();
        self.layout();
        self.focus_active();
        self.snapshot();
    }
    fn enforce_memory_budget(&mut self) {
        while self
            .tabs
            .iter()
            .filter(|t| t.view.is_some() && t.downloading.get() == 0)
            .count()
            > 2
        {
            if let Some(t) = self
                .tabs
                .iter_mut()
                .filter(|t| t.id != self.active && t.view.is_some() && t.downloading.get() == 0)
                .min_by_key(|t| t.used)
            {
                t.view.take();
            } else {
                break;
            }
        }
    }
    fn ensure_view(&mut self) -> Result<(), String> {
        let idx = self.tabs.iter().position(|t| t.id == self.active).unwrap();
        if self.tabs[idx].view.is_some() || self.tabs[idx].url == "yas://newtab" {
            return Ok(());
        }
        let id = self.active;
        let source = self.tabs[idx].source.clone();
        *source.borrow_mut() = self.tabs[idx].url.clone();
        let nav_source = source.clone();
        let p = self.proxy.clone();
        let p2 = p.clone();
        let p3 = p.clone();
        let p4 = p.clone();

        let bounds = self.bounds();
        let private = self.tabs[idx].private;
        let profile = if private {
            format!("YasStealth{}", self.stealth_session)
        } else {
            identity::regular_profile(&self.root).into()
        };
        let keeper = if private {
            &mut self.stealth_keeper
        } else {
            &mut self.regular_keeper
        };
        if keeper.is_none() {
            *keeper = Some(
                WebViewBuilder::new()
                    .with_environment(self.shell.environment())
                    .with_profile_name(profile.clone())
                    .with_incognito(private)
                    .with_visible(false)
                    .with_focused(false)
                    .with_bounds(Rect {
                        position: LogicalPosition::new(0., 0.).into(),
                        size: LogicalSize::new(1., 1.).into(),
                    })
                    .with_url("about:blank")
                    .with_navigation_handler(|u| u == "about:blank")
                    .build_as_child(&self.window)
                    .map_err(|e| e.to_string())?,
            );
        }
        let view = WebViewBuilder::new()
            .with_environment(self.shell.environment())
            .with_profile_name(profile)
            .with_url("about:blank")
            .with_bounds(bounds)
            .with_visible(false)
            .with_general_autofill_enabled(!private)
            .with_incognito(private)
            .with_devtools(true)
            .with_hotkeys_zoom(true)
            .with_navigation_handler(move |u| {
                if u == "about:blank" {
                    return true;
                }
                let allowed =
                    url::Url::parse(&u).is_ok_and(|v| matches!(v.scheme(), "http" | "https"));
                if allowed {
                    *nav_source.borrow_mut() = u.clone();
                    let _ = p.send_event(Msg::Navigate(id, u));
                }
                allowed
            })
            .with_document_title_changed_handler(move |t| {
                let _ = p2.send_event(Msg::Title(id, t));
            })
            .with_on_page_load_handler(move |e, u| {
                if matches!(e, PageLoadEvent::Finished) {
                    let _ = p3.send_event(Msg::Loaded(id, u));
                }
            })
            .build_as_child(&self.window)
            .map_err(|e| e.to_string())?;
        shortcuts::attach(&view, self.proxy.clone()).map_err(|e| e.to_string())?;
        downloads::attach(
            &view,
            self.downloads.clone(),
            id,
            private,
            self.tabs[idx].downloading.clone(),
            self.proxy.clone(),
        )
        .map_err(|e| e.to_string())?;
        // SourceChanged also covers History API and fragment navigation, which
        // do not necessarily finish a new document load. Read the current view
        // when processing the event so a retired view cannot publish its URL.
        let source_proxy = self.proxy.clone();
        unsafe {
            let mut source_token = 0;
            view.webview()
                .add_SourceChanged(
                    &SourceChangedEventHandler::create(Box::new(move |_, _| {
                        let _ = source_proxy.send_event(Msg::RefreshUrl(id));
                        Ok(())
                    })),
                    &mut source_token,
                )
                .map_err(|e| e.to_string())?;
        }
        let popup_source = source.clone();
        let popup_shield = self.shield.clone();
        unsafe {
            let mut token = 0;
            view.webview()
                .add_NewWindowRequested(
                    &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                        let Some(args) = args else { return Ok(()) };
                        args.SetHandled(true)?;
                        let mut initiated = windows::core::BOOL::default();
                        args.IsUserInitiated(&mut initiated)?;
                        if !initiated.as_bool() {
                            return Ok(());
                        }
                        let mut raw = PWSTR::null();
                        args.Uri(&mut raw)?;
                        let uri = take_pwstr(raw);
                        if !url::Url::parse(&uri)
                            .is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
                        {
                            return Ok(());
                        }
                        let source = popup_source.borrow();
                        let shield = popup_shield.borrow();
                        if shield.enabled(&source)
                            && adblock::request::Request::new(&uri, &source, "popup", "GET")
                                .is_ok_and(|r| {
                                    shield.engine.check_network_request(&r).should_block()
                                })
                        {
                            return Ok(());
                        }
                        let _ = p4.send_event(Msg::Popup(uri, private));
                        Ok(())
                    })),
                    &mut token,
                )
                .map_err(|e| e.to_string())?;
        }
        blocker::attach(
            &view,
            self.shield.clone(),
            source,
            self.tabs[idx].blocked.clone(),
        )
        .map_err(|e| e.to_string())?;
        unsafe {
            if let Ok(s) = view
                .webview()
                .Settings()
                .and_then(|s| s.cast::<ICoreWebView2Settings4>())
            {
                let _ = s.SetIsPasswordAutosaveEnabled(!private);
            }
        }
        view.load_url(&self.tabs[idx].url)
            .map_err(|e| e.to_string())?;
        self.tabs[idx].view = Some(view);
        self.tabs[idx].suspend_requested.set(false);
        Ok(())
    }
    fn navigate(&mut self, input: &str) {
        let dest = destination(input);
        self.panel = false;
        self.suggestions_open = false;
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == self.active) {
            t.url = dest.clone();
            t.blocked.set(0);
            if dest == "yas://newtab" {
                t.view.take();
                t.title = "New tab".into();
            } else if let Some(v) = &t.view {
                let _ = v.load_url(&dest);
            }
        }
        if let Err(e) = self.ensure_view() {
            self.toast(&e)
        }
        self.enforce_memory_budget();
        self.layout();
        self.snapshot();
        self.save();
    }
    fn profile(&self) -> Result<ICoreWebView2Profile7, String> {
        let v = self
            .active_tab()
            .filter(|t| !t.private)
            .and_then(|t| t.view.as_ref())
            .ok_or("Open a regular website tab first")?;
        unsafe {
            v.webview()
                .cast::<ICoreWebView2_13>()
                .and_then(|w| w.Profile())
                .and_then(|p| p.cast())
                .map_err(|e| e.to_string())
        }
    }
    fn extensions(&self) {
        let p = self.proxy.clone();
        let result = (|| -> Result<(), String> {
            unsafe {
                self.profile()?
                    .GetBrowserExtensions(&ProfileGetBrowserExtensionsCompletedHandler::create(
                        Box::new(move |hr, list| {
                            let mut entries = Vec::new();
                            if hr.is_ok() {
                                if let Some(list) = list {
                                    let mut count = 0;
                                    list.Count(&mut count)?;
                                    for i in 0..count {
                                        let e = list.GetValueAtIndex(i)?;
                                        let mut id = PWSTR::null();
                                        let mut name = PWSTR::null();
                                        e.Id(&mut id)?;
                                        e.Name(&mut name)?;
                                        entries.push(
                                            json!({"id":take_pwstr(id),"name":take_pwstr(name)}),
                                        );
                                    }
                                }
                            }
                            let _ = p.send_event(Msg::Extensions(entries));
                            Ok(())
                        }),
                    ))
                    .map_err(|e| e.to_string())
            }
        })();
        if let Err(e) = result {
            self.toast(&e)
        }
    }
    fn load_extension(&self, path: PathBuf) {
        if !path.join("manifest.json").is_file() {
            self.toast("This folder has no manifest.json");
            return;
        }
        let proxy = self.proxy.clone();
        let result = (|| -> Result<(), String> {
            unsafe {
                self.profile()?
                    .AddBrowserExtension(
                        &HSTRING::from(path.as_os_str()),
                        &ProfileAddBrowserExtensionCompletedHandler::create(Box::new(
                            move |hr, _| {
                                let _ = proxy.send_event(Msg::Toast(if hr.is_ok() {
                                    "Extension loaded. Reload website tabs to activate it.".into()
                                } else {
                                    format!("Extension failed: {hr:?}")
                                }));
                                Ok(())
                            },
                        )),
                    )
                    .map_err(|e| e.to_string())
            }
        })();
        if let Err(e) = result {
            self.toast(&e)
        }
    }
    fn update_filters(&mut self) {
        if self.updating {
            return;
        }
        self.updating = true;
        self.snapshot();
        let root = self.root.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<String, String> {
                for (name, url) in blocker::SOURCES {
                    let mut response = ureq::get(url).call().map_err(|e| e.to_string())?;
                    let body = response
                        .body_mut()
                        .with_config()
                        .limit(16 * 1024 * 1024)
                        .read_to_string()
                        .map_err(|e| e.to_string())?;
                    if !body.starts_with("[Adblock") || body.lines().count() < 100 {
                        return Err("Invalid filter list; existing rules kept".into());
                    }
                    atomic_write(&root.join(format!("{name}.pending")), body.as_bytes())?;
                }
                for (name, _) in blocker::SOURCES {
                    let bytes = fs::read(root.join(format!("{name}.pending")))
                        .map_err(|e| e.to_string())?;
                    atomic_write(&root.join(name), &bytes)?;
                }
                Ok("EasyList and EasyPrivacy updated".into())
            })();
            let _ = proxy.send_event(Msg::Filters(result));
        });
    }
    fn ui(&mut self, v: Value) {
        let text = |key: &str| v[key].as_str().unwrap_or_default().to_owned();
        let cmd = text("cmd");
        if matches!(
            cmd.as_str(),
            "import" | "save_login" | "delete_login" | "import_browser_data"
        ) {
            self.login_sites.replace(None);
        }
        match cmd.as_str() {
            "suggest" => self.suggest(&text("query")),
            "suggestions_open" => {
                self.suggestions_open = v["open"].as_bool().unwrap_or(false);
                self.suggestions_height = v["height"]
                    .as_f64()
                    .filter(|h| h.is_finite())
                    .unwrap_or(TOP + 352.)
                    .clamp(TOP, TOP + 352.);
                self.layout();
            }
            "window_drag" => {
                let _ = self.window.drag_window();
            }
            "window_minimize" => self.window.set_minimized(true),
            "window_maximize" => {
                self.window.set_maximized(!self.window.is_maximized());
                self.snapshot();
            }
            "window_close" => {
                self.save_now();
                let _ = self.proxy.send_event(Msg::CloseWindow);
            }
            "quit" => {
                let _ = self.proxy.send_event(Msg::Exit);
            }
            "background_private_downloads" => {
                self.settings.background_private_downloads =
                    v["enabled"].as_bool().unwrap_or(false);
                self.save();
                self.update_tray();
                self.snapshot();
            }
            "clear_finished_downloads" => {
                downloads::clear_finished(&self.downloads);
                self.persist_downloads(true);
                self.snapshot();
            }
            "focus_address" => {
                self.panel = false;
                self.layout();
                let _ = self.shell.focus();
                let _ = self.shell.evaluate_script("focusAddress()");
            }
            "close_current" => self.ui(json!({"cmd":"close","id":self.active})),
            "next_tab" => {
                if let Some(i) = self.tabs.iter().position(|t| t.id == self.active) {
                    self.switch(self.tabs[(i + 1) % self.tabs.len()].id);
                }
            }
            "previous_tab" => {
                if let Some(i) = self.tabs.iter().position(|t| t.id == self.active) {
                    self.switch(self.tabs[(i + self.tabs.len() - 1) % self.tabs.len()].id);
                }
            }
            "history" | "downloads" => {
                self.panel = true;
                self.layout();
                let _ = self.shell.focus();
                let _ = self
                    .shell
                    .evaluate_script(&format!("showPanel({})", json!(cmd)));
            }
            "ready" => {
                self.library_dirty.set(true);
                self.snapshot();
                self.layout();
                if let Some(error) = &self.download_error {
                    self.toast(&format!("Download history needs attention: {error}"));
                }
            }
            "new" => {
                let private = self.active_tab().is_some_and(|t| t.private);
                self.add("yas://newtab", private)
            }
            "new_regular" => self.add("yas://newtab", false),
            "private" | "stealth" => self.add("yas://newtab", true),
            "switch" => self.switch(v["id"].as_u64().unwrap_or_default() as usize),
            "close" => {
                let id = v["id"].as_u64().unwrap_or_default() as usize;
                downloads::cancel_tab(&self.downloads, id);
                self.tabs.retain(|t| t.id != id);
                if !self.tabs.iter().any(|t| t.private) {
                    self.stealth_keeper.take();
                    self.stealth_session += 1;
                    downloads::forget_private_finished(&self.downloads);
                }
                if self.tabs.is_empty() {
                    self.add("yas://newtab", false)
                } else if self.active == id {
                    self.switch(self.tabs.last().unwrap().id)
                }
                self.snapshot();
                self.save();
                self.persist_downloads(false);
                self.update_tray();
            }
            "navigate" => self.navigate(&text("url")),
            "back" | "forward" | "reload" | "stop" | "devtools" | "zoom_in" | "zoom_out"
            | "zoom_reset" | "print" | "find" => {
                if let Some(view) = self.active_tab().and_then(|t| t.view.as_ref()) {
                    match cmd.as_str() {
                        "back" => {
                            let _ = view.go_back();
                        }
                        "forward" => {
                            let _ = view.go_forward();
                        }
                        "reload" => {
                            let _ = view.reload();
                        }
                        "stop" => unsafe {
                            let _ = view.webview().Stop();
                        },
                        "devtools" => view.open_devtools(),
                        "zoom_in" | "zoom_out" | "zoom_reset" => unsafe {
                            let c = view.controller();
                            let mut f = 1.;
                            let _ = c.ZoomFactor(&mut f);
                            let _ = c.SetZoomFactor(if cmd == "zoom_reset" {
                                1.
                            } else {
                                (f + if cmd == "zoom_in" { 0.1 } else { -0.1 }).clamp(0.25, 3.)
                            });
                        },
                        "print" => {
                            let _ = view.evaluate_script("window.print()");
                        }
                        "find" => {
                            let _ = view
                                .evaluate_script(&format!("window.find({})", json!(text("query"))));
                        }
                        _ => (),
                    }
                }
            }
            "panel" => {
                self.suggestions_open = false;
                self.panel = v["open"].as_bool().unwrap_or(false);
                if !self.panel && !self.background {
                    if let Err(error) = self.ensure_view() {
                        self.toast(&error);
                    }
                }
                self.layout();
                if self.panel {
                    self.snapshot();
                }
            }
            "bookmark" => {
                if let Some(t) = self.active_tab() {
                    let e = Entry {
                        title: t.title.clone(),
                        url: t.url.clone(),
                        ..Entry::default()
                    };
                    if e.url != "yas://newtab" {
                        if self.settings.bookmarks.iter().any(|b| b.url == e.url) {
                            self.settings.bookmarks.retain(|b| b.url != e.url);
                        } else {
                            self.settings.bookmarks.push(e);
                        }
                        self.save();
                        self.library_dirty.set(true);
                        self.snapshot();
                    }
                }
            }
            "delete_bookmark" => {
                let u = text("url");
                self.settings.bookmarks.retain(|b| b.url != u);
                self.library_dirty.set(true);
                self.save();
                self.snapshot();
            }
            "clear_history" => {
                self.settings.history.clear();
                self.library_dirty.set(true);
                self.save();
                self.snapshot();
            }
            "sleep" => {
                for t in &mut self.tabs {
                    if t.id != self.active && t.downloading.get() == 0 {
                        t.view.take();
                    }
                }
                self.snapshot();
                self.toast("Background tabs released from memory");
            }
            "shield" => {
                let host = self
                    .active_tab()
                    .and_then(|t| url::Url::parse(&t.url).ok())
                    .and_then(|u| u.host_str().map(str::to_owned));
                if let Some(h) = host {
                    if self.settings.exceptions.contains(&h) {
                        self.settings.exceptions.retain(|x| x != &h);
                    } else {
                        self.settings.exceptions.push(h);
                    }
                    self.shield.borrow_mut().exceptions = self.settings.exceptions.clone();
                    self.save();
                    self.snapshot();
                    self.toast("Site protection changed. Reload to apply cosmetic changes.");
                }
            }
            "update_filters" => self.update_filters(),
            "custom_filters" => {
                self.settings.custom_filters = text("rules");
                let count = self.shield.borrow().count;
                let mut s = blocker::Shield::load(
                    &self.root,
                    &self.settings.custom_filters,
                    self.settings.exceptions.clone(),
                );
                s.count = count;
                *self.shield.borrow_mut() = s;
                self.save();
                self.snapshot();
                self.toast("Rules saved. Reload the page to apply.");
            }
            "vault" => self.vault_metadata(),
            "capture_png" | "capture_pdf" => {
                if self.active_tab().and_then(|t| t.view.as_ref()).is_none() {
                    self.toast("Open a webpage before exporting it.");
                    return;
                }
                // Surface capture needs the page visible, including when invoked from Settings.
                self.panel = false;
                let _ = self.shell.evaluate_script("hidePanel()");
                self.layout();
                self.focus_active();
                let png = cmd == "capture_png";
                let extension = if png { "png" } else { "pdf" };
                if let Some(path) = rfd::FileDialog::new()
                    .set_parent(&self.window)
                    .set_title(if png {
                        "Save full-page PNG"
                    } else {
                        "Save page as PDF"
                    })
                    .set_file_name(format!("Yas-page.{extension}"))
                    .add_filter(if png { "PNG image" } else { "PDF document" }, &[extension])
                    .save_file()
                {
                    let view = self.active_tab().and_then(|t| t.view.as_ref()).unwrap();
                    let result = if png {
                        capture::save_png(view, path, self.proxy.clone())
                    } else {
                        capture::save_pdf(view, path, self.proxy.clone())
                    };
                    match result {
                        Ok(()) => self.toast("Exporting the full page…"),
                        Err(e) => self.toast(&e),
                    }
                }
                self.focus_active();
            }
            "import" => {
                if let Some(path) = rfd::FileDialog::new()
                    .set_parent(&self.window)
                    .add_filter("Browser password CSV", &["csv"])
                    .pick_file()
                {
                    let result = (|| {
                        let mut vault = read_vault(&self.root)?;
                        let n = import_logins(path, &mut vault)?;
                        write_vault(&self.root, &vault)?;
                        Ok::<_, String>(n)
                    })();
                    match result {
                        Ok(n) => self.toast(&format!(
                            "Imported {n} logins. Delete the plaintext CSV when finished."
                        )),
                        Err(e) => self.toast(&e),
                    }
                    self.vault_metadata();
                }
            }
            "save_login" => {
                let result = (|| {
                    let site = origin(&text("origin")).ok_or("Use a full HTTPS website address")?;
                    if text("password").is_empty() {
                        return Err("Enter a password".into());
                    }
                    let mut vault = read_vault(&self.root)?;
                    let username = text("username");
                    vault
                        .logins
                        .retain(|l| l.origin != site || l.username != username);
                    vault.logins.push(Login {
                        origin: site,
                        username,
                        password: text("password"),
                        label: None,
                    });
                    write_vault(&self.root, &vault)
                })();
                match result {
                    Ok(_) => {
                        self.toast("Login encrypted for your Windows account");
                        self.vault_metadata()
                    }
                    Err(e) => self.toast(&e),
                }
            }
            "delete_login" => {
                if let Ok(mut vault) = read_vault(&self.root) {
                    let site = text("origin");
                    let user = text("username");
                    vault
                        .logins
                        .retain(|l| l.origin != site || l.username != user);
                    if let Err(e) = write_vault(&self.root, &vault) {
                        self.toast(&e)
                    }
                    self.vault_metadata();
                }
            }
            "save_address" => {
                let result = (|| {
                    let mut vault = read_vault(&self.root)?;
                    vault.address =
                        serde_json::from_value(v["address"].clone()).map_err(|e| e.to_string())?;
                    if vault.address != Address::default()
                        && !vault.addresses.contains(&vault.address)
                    {
                        vault.addresses.push(vault.address.clone());
                    }
                    write_vault(&self.root, &vault)
                })();
                if let Err(e) = result {
                    self.toast(&e)
                } else {
                    self.toast("Address saved")
                }
            }
            "fill_login" => self.fill_login(&text("origin"), &text("username")),
            "fill_address" => self.fill_address(),
            "choose_address" => {
                let result = (|| -> Result<(), String> {
                    let mut vault = read_vault(&self.root)?;
                    vault.address = vault
                        .addresses
                        .get(v["index"].as_u64().unwrap_or(usize::MAX as u64) as usize)
                        .ok_or("Autofill profile not found")?
                        .clone();
                    write_vault(&self.root, &vault)
                })();
                if let Err(e) = result {
                    self.toast(&e)
                } else {
                    self.vault_metadata();
                }
            }
            "fill_form_entry" => {
                self.fill_form_entry(v["index"].as_u64().unwrap_or(usize::MAX as u64) as usize)
            }
            "show_login" => {
                if let Ok(vault) = read_vault(&self.root) {
                    if let Some(login) = vault
                        .logins
                        .iter()
                        .find(|l| l.origin == text("origin") && l.username == text("username"))
                    {
                        let _ = self.shell.evaluate_script(&format!(
                            "showLogin({},{},{})",
                            json!(login.origin),
                            json!(login.username),
                            json!(login.password)
                        ));
                    }
                }
            }
            "import_browser_data" => {
                if let Some(path) = rfd::FileDialog::new()
                    .set_parent(&self.window)
                    .set_title("Import browser data")
                    .add_filter("Browser export JSON", &["json"])
                    .pick_file()
                {
                    self.save_now();
                    match migration::import(&self.root, &path, None) {
                        Ok(report) => {
                            match read_settings(&self.root) {
                                Ok(settings) => {
                                    self.settings = settings;
                                    self.settings_writable = true;
                                }
                                Err(error) => {
                                    self.settings_writable = false;
                                    self.toast(&error);
                                    return;
                                }
                            }
                            self.library_dirty.set(true);
                            self.snapshot();
                            self.toast(&format!(
                                "Imported {} bookmarks, {} history pages and {} autofill values",
                                report.bookmarks_added,
                                report.history_added,
                                report.form_entries_added
                            ));
                        }
                        Err(e) => self.toast(&e),
                    }
                }
            }
            "extensions" => self.extensions(),
            "add_extension" => {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Select an unpacked extension folder")
                    .pick_folder()
                {
                    self.load_extension(path);
                }
            }
            "remove_extension" => {
                let id = text("id");
                let p = self.proxy.clone();
                let result = (|| -> Result<(), String> {
                    unsafe {
                        self.profile()?.GetBrowserExtensions(&ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(move|hr,list|{if hr.is_ok(){if let Some(list)=list{let mut n=0;list.Count(&mut n)?;for i in 0..n{let e=list.GetValueAtIndex(i)?;let mut raw=PWSTR::null();e.Id(&mut raw)?;if take_pwstr(raw)==id{let p=p.clone();e.Remove(&BrowserExtensionRemoveCompletedHandler::create(Box::new(move|hr|{let _=p.send_event(Msg::Toast(if hr.is_ok(){"Extension removed".into()}else{"Could not remove extension".into()}));Ok(())})))?;}}}}Ok(())}))).map_err(|e|e.to_string())
                    }
                })();
                if let Err(e) = result {
                    self.toast(&e)
                }
            }
            "cancel_download" => {
                if let Err(e) =
                    downloads::cancel(&self.downloads, v["id"].as_u64().unwrap_or(0) as usize)
                {
                    self.toast(&e)
                }
                self.enforce_memory_budget();
                self.persist_downloads(true);
                self.update_tray();
                self.snapshot();
            }
            "pause_download" | "resume_download" => {
                let id = v["id"].as_u64().unwrap_or(0) as usize;
                let result = if cmd == "pause_download" {
                    downloads::pause(&self.downloads, id)
                } else {
                    downloads::resume(&self.downloads, id)
                };
                if let Err(e) = result {
                    self.toast(&e)
                }
                self.enforce_memory_budget();
                self.layout();
                self.persist_downloads(true);
                self.update_tray();
                self.snapshot();
            }
            "open_download" | "reveal_download" => {
                let result = (|| -> Result<(), String> {
                    let path = {
                        let manager = self.downloads.borrow();
                        manager
                            .records
                            .iter()
                            .find(|r| {
                                r.id == v["id"].as_u64().unwrap_or(0) as usize
                                    && r.status == "completed"
                            })
                            .and_then(|r| r.path.as_ref())
                            .map(PathBuf::from)
                            .ok_or("Choose a completed download")?
                    };
                    if !path.is_absolute() || !path.is_file() {
                        return Err("The downloaded file is no longer available".into());
                    }
                    if cmd == "reveal_download" {
                        std::process::Command::new("explorer.exe")
                            .arg(path.parent().ok_or("The file has no folder")?)
                            .spawn()
                            .map_err(|e| format!("Could not show the folder: {e}"))?;
                    } else {
                        use windows::Win32::{
                            UI::Shell::ShellExecuteW, UI::WindowsAndMessaging::SW_SHOWNORMAL,
                        };
                        let opened = unsafe {
                            ShellExecuteW(
                                None,
                                &HSTRING::from("open"),
                                &HSTRING::from(path.as_os_str()),
                                None,
                                None,
                                SW_SHOWNORMAL,
                            )
                        };
                        if opened.0 as isize <= 32 {
                            return Err(
                                "Windows could not open this file. Use Show folder to locate it."
                                    .into(),
                            );
                        }
                    }
                    Ok(())
                })();
                if let Err(e) = result {
                    self.toast(&e)
                }
            }
            _ => (),
        }
    }
    fn suggest(&self, query: &str) {
        let items = if query.trim().chars().count() < 2 || query.len() > 512 {
            Vec::new()
        } else {
            if self.login_sites.borrow().is_none() {
                let sites = read_vault(&self.root)
                    .map(|vault| {
                        let mut seen = std::collections::HashSet::new();
                        vault
                            .logins
                            .into_iter()
                            .map(|l| l.origin)
                            .filter(|s| seen.insert(s.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                self.login_sites.replace(Some(sites));
            }
            let sites = self.login_sites.borrow();
            suggestions::search(
                &self.settings,
                sites.as_deref().unwrap_or_default(),
                query,
                self.active_tab().is_some_and(|t| t.private),
            )
        };
        let _ = self.shell.evaluate_script(&format!(
            "suggestionData({},{})",
            json!(query),
            json!(items)
        ));
    }
    fn vault_metadata(&self) {
        match read_vault(&self.root) {
            Ok(v) => {
                let logins: Vec<_> = v
                    .logins
                    .iter()
                    .map(|l| json!({"origin":l.origin,"username":l.username,"label":l.label}))
                    .collect();
                let _ = self.shell.evaluate_script(&format!(
                    "vaultData({})",
                    json!({"logins":logins,"address":v.address,"addresses":v.addresses,"form_entries":v.form_entries})
                ));
            }
            Err(e) => self.toast(&format!("Vault could not be unlocked: {e}")),
        }
    }
    fn fill_form_entry(&mut self, index: usize) {
        let result = (|| -> Result<(), String> {
            let view = self
                .active_tab()
                .and_then(|t| t.view.as_ref())
                .ok_or("Open a website first")?;
            let site = origin(&view.url().map_err(|e| e.to_string())?)
                .ok_or("Saved form filling requires HTTPS")?;
            let vault = read_vault(&self.root)?;
            let entry = vault
                .form_entries
                .get(index)
                .ok_or("Saved form entry not found")?;
            if entry.name.is_empty() {
                return Err("This saved value has no field name to match".into());
            }
            let script=format!("(()=>{{if(location.origin!=={})return false;const name={},value={};const visible={};const allowed=['text','email','tel','search','url','number','date','month','week','datetime-local','time'];const candidates=[...document.querySelectorAll('input')].filter(e=>e.name===name&&allowed.includes(e.type)&&visible(e));const e=candidates.includes(document.activeElement)?document.activeElement:candidates[0];if(!e)return false;Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(e,value);e.dispatchEvent(new Event('input',{{bubbles:true}}));e.dispatchEvent(new Event('change',{{bubbles:true}}));e.focus();return true;}})()",json!(site),json!(entry.name),json!(entry.value),VISIBLE_FIELD_JS);
            let proxy = self.proxy.clone();
            view.evaluate_script_with_callback(&script, move |result| {
                let filled = serde_json::from_str::<bool>(&result).unwrap_or(false);
                let _ = proxy.send_event(Msg::Toast(
                    if filled {
                        "Saved form value filled"
                    } else {
                        "No matching visible field was found; no saved value was filled"
                    }
                    .into(),
                ));
            })
            .map_err(|e| e.to_string())?;
            Ok(())
        })();
        if let Err(e) = result {
            self.toast(&e)
        } else {
            self.panel = false;
            self.layout();
            self.focus_active();
        }
    }
    fn fill_login(&mut self, site: &str, user: &str) {
        let result = (|| -> Result<(), String> {
            let view = self
                .active_tab()
                .and_then(|t| t.view.as_ref())
                .ok_or("Open a website first")?;
            let current = view.url().map_err(|e| e.to_string())?;
            if origin(&current).as_deref() != Some(site) {
                return Err("This login belongs to a different HTTPS origin".into());
            }
            let vault = read_vault(&self.root)?;
            let login = vault
                .logins
                .iter()
                .find(|l| l.origin == site && l.username == user)
                .ok_or("Login not found")?;
            let script = format!(
                r#"(()=>{{
                if(location.origin!=={site})return{{error:'origin'}};
                const visible={visible};
                const set=(e,value)=>{{if(!visible(e))return false;Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(e,value);e.dispatchEvent(new Event('input',{{bubbles:true}}));e.dispatchEvent(new Event('change',{{bubbles:true}}));return true}};
                const p=[...document.querySelectorAll('input[type=password]')].find(visible);
                const scope=p?.form||document;
                const selectors='input[autocomplete~="username"],input[type=email],input[name*=user i],input[name*=login i],input[name*=email i],input[id*=login i]';
                const eligible=e=>visible(e)&&['text','email','tel'].includes(e.type);
                const u=[...scope.querySelectorAll(selectors)].find(eligible)||(p?[...scope.querySelectorAll('input[type=text]')].find(eligible):null);
                const username=set(u,{username}),password=set(p,{password});
                if(password)p.focus();else if(username)u.focus();
                return{{username,password}};
            }})()"#,
                site = json!(site),
                visible = VISIBLE_FIELD_JS,
                username = json!(login.username),
                password = json!(login.password)
            );
            let proxy = self.proxy.clone();
            view.evaluate_script_with_callback(&script, move |result| {
                let fields = serde_json::from_str::<Value>(&result).unwrap_or(Value::Null);
                let message = if fields["error"] == "origin" {
                    "The page changed; no credentials were filled"
                } else if fields["password"].as_bool() == Some(true) {
                    "Login filled on the matching HTTPS site"
                } else if fields["username"].as_bool() == Some(true) {
                    "Username filled. Continue, then choose Fill for the password"
                } else {
                    "No matching visible login field was found on this page"
                };
                let _ = proxy.send_event(Msg::Toast(message.into()));
            })
            .map_err(|e| e.to_string())?;
            Ok(())
        })();
        if let Err(e) = result {
            self.toast(&e)
        } else {
            self.panel = false;
            self.suggestions_open = false;
            self.layout();
            self.focus_active();
        }
    }
    fn fill_address(&mut self) {
        let result = (|| -> Result<(), String> {
            let view = self
                .active_tab()
                .and_then(|t| t.view.as_ref())
                .ok_or("Open a website first")?;
            let site = origin(&view.url().map_err(|e| e.to_string())?)
                .ok_or("Address autofill requires HTTPS")?;
            let a = read_vault(&self.root)?.address;
            if a == Address::default() {
                return Err("No address profile is saved. Your imported values are available under Saved form entries.".into());
            }
            let fields = json!({"name":a.name,"email":a.email,"tel":a.phone,"street-address":a.street,"address-line1":a.street,"address-level2":a.city,"address-level1":a.region,"postal-code":a.postal,"country-name":a.country});
            let script=format!("(()=>{{if(location.origin!=={})return 0;const data={};const visible={};let filled=0;for(const e of document.querySelectorAll('input[autocomplete],textarea[autocomplete]')){{const k=e.autocomplete.split(' ').pop();if(data[k]&&visible(e)&&(e.tagName==='TEXTAREA'||['text','email','tel','number'].includes(e.type))){{const p=e.tagName==='TEXTAREA'?HTMLTextAreaElement.prototype:HTMLInputElement.prototype;Object.getOwnPropertyDescriptor(p,'value').set.call(e,data[k]);e.dispatchEvent(new Event('input',{{bubbles:true}}));e.dispatchEvent(new Event('change',{{bubbles:true}}));filled++;}}}}return filled;}})()",json!(site),fields,VISIBLE_FIELD_JS);
            let proxy = self.proxy.clone();
            view.evaluate_script_with_callback(&script, move |result| {
                let count = serde_json::from_str::<usize>(&result).unwrap_or(0);
                let _ = proxy.send_event(Msg::Toast(if count > 0 {
                    format!("Filled {count} address fields")
                } else {
                    "No matching visible address fields were found".into()
                }));
            })
            .map_err(|e| e.to_string())?;
            Ok(())
        })();
        if let Err(e) = result {
            self.toast(&e)
        } else {
            self.panel = false;
            self.layout();
            self.focus_active();
        }
    }
}
fn main() {
    if let Err(e) = run() {
        if std::env::args().any(|a| a == "--import-data") {
            eprintln!("Import failed: {e}");
            std::process::exit(1);
        }
        rfd::MessageDialog::new()
            .set_title("Yas could not start")
            .set_description(e.to_string())
            .show();
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let root = if let Some(i) = args.iter().position(|s| s == "--user-data-dir") {
        PathBuf::from(
            args.get(i + 1)
                .filter(|s| !s.starts_with("--"))
                .ok_or("--user-data-dir requires a directory path")?,
        )
    } else {
        identity::prepare_default_root()?
    };
    fs::create_dir_all(&root)?;
    let _profile_lock = match identity::lock_profile(&root) {
        Ok(lock) => lock,
        Err(error) => {
            let activation_only = !args.iter().any(|arg| {
                matches!(
                    arg.as_str(),
                    "--import-data" | "--import-passwords" | "--import-report" | "--load-extension"
                )
            });
            if activation_only {
                if background::request_existing(&root) {
                    return Ok(());
                }
                // A background owner can finish its final transfer while the
                // activation is in flight. Start normally once it releases
                // the lock instead of losing the user's reopen request.
                let mut replacement = None;
                for _ in 0..20 {
                    if let Ok(lock) = identity::lock_profile(&root) {
                        replacement = Some(lock);
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                if let Some(lock) = replacement {
                    lock
                } else {
                    return Err(error.into());
                }
            } else {
                return Err(error.into());
            }
        }
    };
    if let Some(i) = args.iter().position(|s| s == "--import-data") {
        let input = PathBuf::from(
            args.get(i + 1)
                .ok_or("--import-data requires a JSON file")?,
        );
        let csv = args
            .iter()
            .position(|s| s == "--import-passwords")
            .map(|i| {
                args.get(i + 1)
                    .map(PathBuf::from)
                    .ok_or("--import-passwords requires a CSV file")
            })
            .transpose()?;
        let report = migration::import(&root, &input, csv.as_deref())?;
        let path = args
            .iter()
            .position(|s| s == "--import-report")
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("last-import-report.json"));
        storage::atomic_write(&path, &serde_json::to_vec_pretty(&report)?)?;
        return Ok(());
    }
    let proxy_slot = Rc::new(RefCell::new(None));
    let mut builder = EventLoopBuilder::<Msg>::with_user_event();
    builder.with_msg_hook(background::hook(proxy_slot.clone()));
    let event_loop = builder.build();
    let proxy = event_loop.create_proxy();
    *proxy_slot.borrow_mut() = Some(proxy.clone());
    let activation = background::Instance::listen(&root, proxy.clone())?;
    let (download_manager, download_warning) = match downloads::Manager::load(&root) {
        Ok(manager) => (manager, None),
        Err(error) => (downloads::Manager::default(), Some(error)),
    };
    let settings = read_settings(&root)?;
    let shield = Rc::new(RefCell::new(blocker::Shield::load(
        &root,
        &settings.custom_filters,
        settings.exceptions.clone(),
    )));
    let window = WindowBuilder::new()
        .with_title("Yas")
        .with_decorations(false)
        .with_window_icon(
            tao::window::Icon::from_rgba(include_bytes!("../branding/icon.rgba").to_vec(), 64, 64)
                .ok(),
        )
        .with_inner_size(LogicalSize::new(1240., 820.))
        .with_min_inner_size(LogicalSize::new(760., 500.))
        .build(&event_loop)?;
    let tray = background::Tray::new(&window)?;
    // Only the website profile receives installed extensions. UI and private profiles stay empty.
    // Remote pages have no privileged IPC handler. Profiles share the engine to reduce overhead.
    let p = proxy.clone();
    let mut shell_context = WebContext::new(Some(root.join("shell")));
    let shell = WebViewBuilder::new_with_web_context(&mut shell_context)
        .with_custom_protocol("yas-ui".into(), |_, _| {
            wry::http::Response::builder()
                .header("Content-Type", "text/html; charset=utf-8")
                .header("Cache-Control", "no-store")
                .body(std::borrow::Cow::Borrowed(
                    include_bytes!("shell.html").as_slice(),
                ))
                .unwrap()
        })
        .with_url("yas-ui://localhost/")
        .with_navigation_handler(|u| {
            u == "http://yas-ui.localhost/" || u == "yas-ui://localhost/" || u == "about:blank"
        })
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_browser_extensions_enabled(true)
        .with_profile_name("YasUI")
        .with_general_autofill_enabled(false)
        .with_ipc_handler(move |req| {
            if let Ok(v) = serde_json::from_str(req.body()) {
                let _ = p.send_event(Msg::Ui(v));
            }
        })
        .build_as_child(&window)?;
    shortcuts::attach(&shell, proxy.clone())?;
    let mut app = Browser {
        window,
        shell,
        regular_keeper: None,
        stealth_keeper: None,
        stealth_session: 0,
        tabs: vec![],
        active: 0,
        next: 1,
        panel: false,
        suggestions_open: false,
        suggestions_height: TOP,
        login_sites: RefCell::new(None),
        root,
        settings,
        shield,
        proxy: proxy.clone(),
        downloads: Rc::new(RefCell::new(download_manager)),
        updating: false,
        settings_dirty: false,
        settings_writable: true,
        library_dirty: Cell::new(true),
        background: false,
        tray,
        tray_tip: None,
        _activation: activation,
        pending_download_prompts: Vec::new(),
        download_error: download_warning,
        exiting: false,
    };
    let session = app.settings.session.clone();
    if session.is_empty() {
        app.add("yas://newtab", false)
    } else {
        // Restore every saved tab as metadata; only the selected page loads.
        // Capping this list would silently erase the rest on the next save.
        for u in session {
            let id = app.next;
            app.next += 1;
            app.tabs.push(Tab {
                id,
                url: destination(&u),
                title: if u == "yas://newtab" {
                    "New tab".into()
                } else {
                    "Restored tab".into()
                },
                view: None,
                source: Rc::new(RefCell::new(String::new())),
                blocked: Rc::new(Cell::new(0)),
                downloading: Rc::new(Cell::new(0)),
                used: Instant::now(),
                private: false,
                suspend_requested: Cell::new(false),
            });
        }
        app.switch(app.tabs[0].id);
    }
    if let Some(i) = args.iter().position(|s| s == "--load-extension") {
        if let Some(path) = args.get(i + 1) {
            if app.active_tab().is_none_or(|t| t.url == "yas://newtab") {
                app.navigate("https://example.com");
            }
            app.load_extension(PathBuf::from(path));
        }
    }
    if fs::metadata(app.root.join("easylist.txt"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_none_or(|age| age > Duration::from_secs(4 * 24 * 3600))
    {
        app.update_filters();
    }
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(2));
        if proxy.send_event(Msg::Tick).is_err() {
            break;
        }
    });
    let mut last_blocked = 0;
    event_loop.run(move |event, _, flow| {
        if matches!(event, Event::LoopDestroyed) {
            app.save_now();
            if app.downloads.try_borrow().is_ok() {
                let _ = downloads::persist(&app.downloads, &app.root, true);
            }
            return;
        }
        if app.exiting {
            *flow = ControlFlow::Exit;
            return;
        }
        *flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                if app.close_window() {
                    *flow = ControlFlow::Exit;
                }
            }
            Event::WindowEvent {
                event: WindowEvent::Focused(true),
                ..
            } => app.focus_active(),
            Event::WindowEvent {
                event: WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. },
                ..
            } => {
                app.layout();
                app.snapshot();
            }
            Event::UserEvent(msg) => match msg {
                Msg::CloseWindow => {
                    if app.close_window() {
                        *flow = ControlFlow::Exit;
                    }
                }
                Msg::Exit => {
                    app.shutdown();
                    *flow = ControlFlow::Exit;
                }
                Msg::Reopen => app.show_window(false),
                Msg::ShowDownloads => app.show_window(true),
                Msg::RestoreTray => {
                    if let Err(error) = app.tray.restore() {
                        app.tray_tip = None;
                        app.show_window(true);
                        app.toast(&error);
                    }
                }
                Msg::DownloadPrompt(id) => {
                    if downloads::can_prompt(&app.downloads, id) {
                        if app.background {
                            if !app.pending_download_prompts.contains(&id) {
                                app.pending_download_prompts.push(id);
                                let _ = app.tray.balloon(
                                    "Choose a download location",
                                    "Reopen Yas to choose where to save this file.",
                                );
                            }
                        } else {
                            downloads::prompt(&app.downloads, id, &app.window, &app.proxy);
                            app.focus_active();
                        }
                        app.persist_downloads(true);
                        app.update_tray();
                    }
                }
                Msg::DownloadChanged => {
                    app.enforce_memory_budget();
                    app.release_background_views();
                    app.persist_downloads(false);
                    app.update_tray();
                    app.snapshot();
                }
                Msg::Ui(v) => app.ui(v),
                Msg::Toast(s) => app.toast(&s),
                Msg::Popup(u, private) => {
                    if !app.background
                        && url::Url::parse(&u).is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
                    {
                        app.add(&u, private);
                    }
                }
                Msg::RefreshUrl(id) => {
                    if let Some(t) = app.tabs.iter_mut().find(|t| t.id == id) {
                        if let Some(u) = t.view.as_ref().and_then(|v| v.url().ok()).filter(|u| {
                            url::Url::parse(u).is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
                        }) {
                            t.url = u.clone();
                            *t.source.borrow_mut() = u;
                        }
                    }
                    app.snapshot();
                    app.save();
                }
                Msg::Navigate(id, u) => {
                    if let Some(t) = app.tabs.iter_mut().find(|t| t.id == id) {
                        t.url = u;
                    }
                    app.snapshot();
                }
                Msg::Title(id, title) => {
                    if let Some(t) = app.tabs.iter_mut().find(|t| t.id == id) {
                        if t.view.is_some() && t.url != "yas://newtab" {
                            t.title = title;
                        }
                    }
                    app.snapshot();
                }
                Msg::Loaded(id, u) => {
                    if let Some(t) = app.tabs.iter().find(|t| t.id == id) {
                        if let Some(view) = &t.view {
                            blocker::cosmetic(view, app.shield.clone(), u.clone());
                        }
                        if !t.private && u.starts_with("http") {
                            let entry = Entry {
                                title: t.title.clone(),
                                url: u,
                                visited_at: Some(
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_millis() as f64,
                                ),
                                ..Entry::default()
                            };
                            app.settings.history.retain(|e| e.url != entry.url);
                            app.settings.history.push(entry);
                            app.library_dirty.set(true);
                        }
                    }
                    app.save();
                    app.snapshot();
                }
                Msg::Filters(result) => {
                    app.updating = false;
                    match result {
                        Ok(s) => {
                            let count = app.shield.borrow().count;
                            let mut shield = blocker::Shield::load(
                                &app.root,
                                &app.settings.custom_filters,
                                app.settings.exceptions.clone(),
                            );
                            shield.count = count;
                            *app.shield.borrow_mut() = shield;
                            app.toast(&s);
                        }
                        Err(e) => app.toast(&format!("Filter update failed: {e}")),
                    }
                    app.snapshot();
                }
                Msg::Tick => {
                    downloads::poll(&app.downloads, &app.proxy);
                    app.release_background_views();
                    app.persist_downloads(false);
                    app.update_tray();
                    if app.settings_dirty {
                        app.save_now();
                    }
                    let mut changed = false;
                    for t in &mut app.tabs {
                        if t.id != app.active
                            && t.downloading.get() == 0
                            && t.used.elapsed() > Duration::from_secs(120)
                        {
                            changed |= t.view.take().is_some();
                        }
                    }
                    let count = app.shield.borrow().count;
                    if changed || count != last_blocked {
                        app.snapshot();
                        last_blocked = count;
                    }
                    if app.background_finished() {
                        *flow = ControlFlow::Exit;
                    }
                }
                Msg::Extensions(e) => {
                    let _ = app
                        .shell
                        .evaluate_script(&format!("extensionData({})", json!(e)));
                }
            },
            _ => (),
        }
    });
}
