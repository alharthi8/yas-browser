mod ota;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cell::Cell,
    fs,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};
use tao::{
    dpi::{LogicalPosition, LogicalSize},
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::{Window, WindowBuilder},
};
use wry::{NewWindowResponse, Rect, WebView, WebViewBuilder};
const TOP: f64 = 106.;
#[derive(Clone, Default, Serialize, Deserialize)]
struct Entry {
    title: String,
    url: String,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    bookmarks: Vec<Entry>,
    history: Vec<Entry>,
    session: Vec<String>,
}
#[derive(Debug)]
enum Msg {
    Ui(Value),
    Navigation(usize, String),
    Title(usize, String),
    Download(String, bool),
    Popup(String, bool),
}
struct Tab {
    id: usize,
    title: String,
    url: String,
    private: bool,
    view: Option<WebView>,
    used: Instant,
    downloads: Rc<Cell<usize>>,
}
struct Browser {
    window: Window,
    shell: WebView,
    tabs: Vec<Tab>,
    active: usize,
    next: usize,
    root: PathBuf,
    saved: Saved,
    proxy: EventLoopProxy<Msg>,
    panel: bool,
    zoom: f64,
}
fn destination(value: &str) -> Option<String> {
    let text = value.trim();
    if text.is_empty() {
        return Some("yas://newtab".into());
    }
    if text == "yas://newtab" {
        return Some(text.into());
    }
    if let Ok(url) = url::Url::parse(text) {
        if matches!(url.scheme(), "https" | "http")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
        {
            return Some(url.into());
        }
        return None;
    }
    if !text.contains(char::is_whitespace) && text.contains('.') && !text.contains(':') {
        return destination(&format!("https://{text}"));
    }
    let mut url = url::Url::parse("https://www.bing.com/search").ok()?;
    url.query_pairs_mut().append_pair("q", text);
    Some(url.into())
}
impl Browser {
    fn active_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == self.active)
    }
    fn persist(&mut self) {
        self.saved.session = self
            .tabs
            .iter()
            .filter(|t| !t.private)
            .map(|t| t.url.clone())
            .collect();
        if let Ok(bytes) = serde_json::to_vec_pretty(&self.saved) {
            let temporary = self.root.join("settings.json.tmp");
            match fs::write(&temporary, bytes)
                .and_then(|_| fs::rename(&temporary, self.root.join("settings.json")))
            {
                Ok(()) => {}
                Err(error) => self.toast(&format!("Settings could not be saved: {error}")),
            }
        }
    }
    fn toast(&self, text: &str) {
        let _ = self
            .shell
            .evaluate_script(&format!("toast({})", json!(text)));
    }
    fn snapshot(&self) {
        let tabs:Vec<_>=self.tabs.iter().map(|t|json!({"id":t.id,"title":t.title,"url":t.url,"private":t.private,"sleeping":t.view.is_none()&&t.url!="yas://newtab"})).collect();
        let state = json!({"tabs":tabs,"active":self.active,"bookmarks":self.saved.bookmarks,"history":self.saved.history.iter().rev().take(200).collect::<Vec<_>>(),"version":env!("CARGO_PKG_VERSION")});
        let _ = self.shell.evaluate_script(&format!("render({state})"));
    }
    fn bounds(&self) -> Rect {
        let scale = self.window.scale_factor();
        let size = self.window.inner_size();
        Rect {
            position: LogicalPosition::new(0., TOP).into(),
            size: LogicalSize::new(
                size.width as f64 / scale,
                (size.height as f64 / scale - TOP).max(1.),
            )
            .into(),
        }
    }
    fn layout(&self) {
        let size = self.window.inner_size();
        let _ = self.shell.set_bounds(Rect {
            position: LogicalPosition::new(0., 0.).into(),
            size: size.into(),
        });
        let bounds = self.bounds();
        for tab in &self.tabs {
            if let Some(view) = &tab.view {
                let _ = view.set_bounds(bounds);
                let _ = view.set_visible(tab.id == self.active && !self.panel);
            }
        }
    }
    fn add(&mut self, url: &str, private: bool) {
        let Some(url) = destination(url) else {
            self.toast("Only HTTP and HTTPS addresses are allowed.");
            return;
        };
        let id = self.next;
        self.next += 1;
        self.tabs.push(Tab {
            id,
            title: "New tab".into(),
            url,
            private,
            view: None,
            used: Instant::now(),
            downloads: Rc::new(Cell::new(0)),
        });
        self.select(id);
    }
    fn select(&mut self, id: usize) {
        if !self.tabs.iter().any(|t| t.id == id) {
            return;
        }
        self.active = id;
        self.panel = false;
        if let Some(tab) = self.active_mut() {
            tab.used = Instant::now();
        }
        if let Err(error) = self.load() {
            self.toast(&error);
        }
        self.layout();
        if let Some(tab) = self.tabs.iter().find(|t| t.id == id) {
            if let Some(view) = &tab.view {
                let _ = view.focus();
            }
        }
        self.trim(false);
        self.persist();
        self.snapshot();
    }
    fn load(&mut self) -> Result<(), String> {
        let index = self
            .tabs
            .iter()
            .position(|t| t.id == self.active)
            .ok_or("No active tab")?;
        let tab = &self.tabs[index];
        if tab.view.is_some() || tab.url == "yas://newtab" {
            return Ok(());
        }
        let id = tab.id;
        let private = tab.private;
        let p = self.proxy.clone();
        let n = self.proxy.clone();
        let title = self.proxy.clone();
        let finished = self.proxy.clone();
        let downloads = tab.downloads.clone();
        let done = tab.downloads.clone();
        let view = WebViewBuilder::new()
            .with_url(&tab.url)
            .with_incognito(private)
            .with_bounds(self.bounds())
            .with_navigation_handler(move |url| {
                let allowed = url::Url::parse(&url).is_ok_and(|u| {
                    matches!(u.scheme(), "http" | "https")
                        && u.host_str().is_some()
                        && u.username().is_empty()
                        && u.password().is_none()
                });
                if allowed {
                    let _ = n.send_event(Msg::Navigation(id, url));
                }
                allowed
            })
            .with_document_title_changed_handler(move |value| {
                let _ = title.send_event(Msg::Title(id, value));
            })
            .with_new_window_req_handler(move |url, _| {
                if destination(&url).is_some() {
                    let _ = p.send_event(Msg::Popup(url, private));
                }
                NewWindowResponse::Deny
            })
            .with_download_started_handler(move |_, path| {
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("download");
                if let Some(chosen) = rfd::FileDialog::new().set_file_name(name).save_file() {
                    *path = chosen;
                    downloads.set(downloads.get() + 1);
                    true
                } else {
                    false
                }
            })
            .with_download_completed_handler(move |url, _, success| {
                done.set(done.get().saturating_sub(1));
                let _ = finished.send_event(Msg::Download(url, success));
            })
            .build_as_child(&self.window)
            .map_err(|e| e.to_string())?;
        // Only the local shell gets an IPC handler. Untrusted websites never receive native commands.
        self.tabs[index].view = Some(view);
        Ok(())
    }
    fn navigate(&mut self, input: &str) {
        let Some(url) = destination(input) else {
            self.toast("Enter an HTTP/HTTPS address or a search.");
            return;
        };
        if let Some(tab) = self.active_mut() {
            tab.url = url;
            tab.title = "Loading…".into();
            tab.view = None;
        }
        self.select(self.active);
    }
    fn close(&mut self, id: usize) {
        if let Some(index) = self.tabs.iter().position(|t| t.id == id) {
            if self.tabs[index].downloads.get() > 0
                && rfd::MessageDialog::new()
                    .set_title("Close this tab?")
                    .set_description("Closing the tab can cancel its active downloads.")
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show()
                    != rfd::MessageDialogResult::Yes
            {
                return;
            }
            self.tabs.remove(index);
            if self.tabs.is_empty() {
                self.add("yas://newtab", false);
            } else if self.active == id {
                self.select(self.tabs[index.min(self.tabs.len() - 1)].id);
            }
            self.layout();
            self.persist();
            self.snapshot();
        }
    }
    fn trim(&mut self, timer: bool) {
        let resident = self.tabs.iter().filter(|t| t.view.is_some()).count();
        let mut eligible: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                t.id != self.active && t.view.is_some() && !t.private && t.downloads.get() == 0
            })
            .map(|(i, t)| (i, t.used))
            .collect();
        eligible.sort_by_key(|(_, time)| *time);
        let mut extra = resident.saturating_sub(2);
        for (index, used) in eligible {
            if extra > 0 || (timer && used.elapsed() > Duration::from_secs(120)) {
                self.tabs[index].view = None;
                extra = extra.saturating_sub(1);
            }
        }
    }
    fn script(&self, script: &str) {
        if let Some(view) = self
            .tabs
            .iter()
            .find(|t| t.id == self.active)
            .and_then(|t| t.view.as_ref())
        {
            let _ = view.evaluate_script(script);
        }
    }
    fn action(&mut self, value: Value) {
        let command = value["cmd"].as_str().unwrap_or("");
        match command {
            "ready" => self.snapshot(),
            "new" => self.add("yas://newtab", false),
            "private" => self.add("yas://newtab", true),
            "select" => {
                if let Some(id) = value["id"].as_u64() {
                    self.select(id as usize);
                }
            }
            "close" => {
                if let Some(id) = value["id"].as_u64() {
                    self.close(id as usize);
                }
            }
            "navigate" => self.navigate(value["url"].as_str().unwrap_or("")),
            "back" => self.script("history.back()"),
            "forward" => self.script("history.forward()"),
            "reload" => self.script("location.reload()"),
            "home" => self.navigate("yas://newtab"),
            "bookmark" => {
                if let Some(tab) = self
                    .tabs
                    .iter()
                    .find(|t| t.id == self.active && t.url.starts_with("http"))
                {
                    let item = Entry {
                        title: tab.title.clone(),
                        url: tab.url.clone(),
                    };
                    if self.saved.bookmarks.iter().any(|e| e.url == item.url) {
                        self.saved.bookmarks.retain(|e| e.url != item.url);
                    } else {
                        self.saved.bookmarks.push(item);
                    }
                    self.persist();
                    self.snapshot();
                }
            }
            "panel" => {
                self.panel = value["open"].as_bool().unwrap_or(false);
                self.layout();
            }
            "clear_history" => {
                self.saved.history.clear();
                self.persist();
                self.snapshot();
            }
            "updates" => ota::check(true),
            "print" => {
                if let Some(view) = self
                    .tabs
                    .iter()
                    .find(|t| t.id == self.active)
                    .and_then(|t| t.view.as_ref())
                {
                    let _ = view.print();
                }
            }
            "zoom_in" | "zoom_out" | "zoom_reset" => {
                self.zoom = if command == "zoom_reset" {
                    1.
                } else {
                    (self.zoom + if command == "zoom_in" { 0.1 } else { -0.1 }).clamp(0.5, 3.)
                };
                if let Some(view) = self
                    .tabs
                    .iter()
                    .find(|t| t.id == self.active)
                    .and_then(|t| t.view.as_ref())
                {
                    let _ = view.zoom(self.zoom);
                }
            }
            _ => {}
        }
    }
}
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    let smoke = args.iter().any(|a| a == "--yas-smoke-test");
    let root = if smoke {
        std::env::temp_dir().join(format!("yas-browser-smoke-{}", std::process::id()))
    } else if let Some(index) = args.iter().position(|s| s == "--user-data-dir") {
        PathBuf::from(args.get(index + 1).ok_or("Missing profile path")?)
    } else {
        directories::ProjectDirs::from("ae", "Appana", "Yas Browser")
            .ok_or("No application data directory")?
            .data_local_dir()
            .to_owned()
    };
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let saved = match fs::read(root.join("settings.json")) {
        Ok(bytes) => serde_json::from_slice::<Saved>(&bytes).map_err(|e| {
            format!("Settings are unreadable; the original file was preserved: {e}")
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Saved::default(),
        Err(e) => return Err(e.to_string()),
    };
    let event_loop = EventLoopBuilder::<Msg>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let p = proxy.clone();
    let window = WindowBuilder::new()
        .with_title("Yas Browser")
        .with_inner_size(LogicalSize::new(1240., 820.))
        .with_min_inner_size(LogicalSize::new(760., 500.))
        .build(&event_loop)
        .map_err(|e| e.to_string())?;
    let html =
        include_str!("shell.html").replace("__YAS_LOGO__", include_str!("../assets/logo.svg"));
    let shell = WebViewBuilder::new()
        .with_html(html)
        .with_navigation_handler(|u| u == "about:blank")
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_ipc_handler(move |request| {
            if request.body().len() < 16384 {
                if let Ok(value) = serde_json::from_str(request.body()) {
                    let _ = p.send_event(Msg::Ui(value));
                }
            }
        })
        .build_as_child(&window)
        .map_err(|e| e.to_string())?;
    let mut app = Browser {
        window,
        shell,
        tabs: vec![],
        active: 0,
        next: 1,
        root,
        saved,
        proxy,
        panel: false,
        zoom: 1.,
    };
    let session = app.saved.session.clone();
    for url in session.iter().take(100) {
        if let Some(url) = destination(url) {
            let id = app.next;
            app.next += 1;
            app.tabs.push(Tab {
                id,
                url,
                title: "Restored tab".into(),
                private: false,
                view: None,
                used: Instant::now(),
                downloads: Rc::new(Cell::new(0)),
            });
        }
    }
    if app.tabs.is_empty() {
        app.add("yas://newtab", false);
    } else {
        app.select(app.tabs[0].id);
    }
    if !smoke {
        ota::check(false);
    }
    let smoke_started = Instant::now();
    let mut smoke_ready = false;
    let mut smoke_page = false;
    let mut tick = Instant::now();
    event_loop.run(move|event,_,flow|{*flow=ControlFlow::WaitUntil(Instant::now()+Duration::from_secs(if smoke{1}else{30}));match event{Event::WindowEvent{event:WindowEvent::CloseRequested,..}=>{app.persist();*flow=ControlFlow::Exit;},Event::WindowEvent{event:WindowEvent::Resized(_)|WindowEvent::ScaleFactorChanged{..},..}=>app.layout(),Event::UserEvent(Msg::Ui(value))=>{if smoke&&value["cmd"]=="ready"{smoke_ready=true;let listener=std::net::TcpListener::bind("127.0.0.1:0").expect("loopback fixture");let address=listener.local_addr().unwrap();std::thread::spawn(move||{use std::io::{Read,Write};for stream in listener.incoming(){let mut stream=stream.unwrap();let mut request=[0;4096];let _=stream.read(&mut request);let body="<!doctype html><title>Yas native smoke fixture</title><p>WKWebView fixture</p>";let _=write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);}});app.add(&format!("http://{address}/"),false);}else if smoke&&value["cmd"]=="smoke_result"{if smoke_ready&&smoke_page&&value["ok"]==true{println!("YAS_NATIVE_SMOKE_OK: shell IPC, DOM rendering, WKWebView navigation/title");std::process::exit(0);}else{std::process::exit(3);}}else{app.action(value);}},Event::UserEvent(Msg::Popup(url,private))=>app.add(&url,private),Event::UserEvent(Msg::Navigation(id,url))=>{if let Some(tab)=app.tabs.iter_mut().find(|t|t.id==id){tab.url=url.clone();if !tab.private{app.saved.history.push(Entry{url,title:tab.title.clone()});let excess=app.saved.history.len().saturating_sub(2000);app.saved.history.drain(..excess);}}app.persist();app.snapshot();},Event::UserEvent(Msg::Title(id,title))=>{if smoke&&title=="Yas native smoke fixture"{smoke_page=true;app.snapshot();let _=app.shell.evaluate_script("window.ipc.postMessage(JSON.stringify({cmd:'smoke_result',ok:document.querySelectorAll('.tab').length===2&&document.querySelector('#address').value.startsWith('http://127.0.0.1:')}))");}if let Some(tab)=app.tabs.iter_mut().find(|t|t.id==id){tab.title=title;}app.snapshot();},Event::UserEvent(Msg::Download(url,success))=>app.toast(&format!("Download {}: {}",if success{"completed"}else{"failed"},url)),Event::MainEventsCleared=>{if smoke&&smoke_started.elapsed()>Duration::from_secs(25){eprintln!("Native smoke timeout");std::process::exit(4);}if tick.elapsed()>=Duration::from_secs(30){app.trim(true);app.snapshot();tick=Instant::now();}},_=>{}}});
}
fn main() {
    if ota::diagnostic() {
        return;
    }
    if let Err(error) = run() {
        rfd::MessageDialog::new()
            .set_title("Yas Browser could not start")
            .set_description(error)
            .show();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blocks_local_and_script_schemes() {
        for input in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,test",
            "https://name:secret@example.com",
        ] {
            assert!(destination(input).is_none());
        }
    }
    #[test]
    fn normalizes_web_and_search() {
        assert_eq!(destination("example.com").unwrap(), "https://example.com/");
        assert!(destination("a useful search")
            .unwrap()
            .contains("bing.com/search"));
    }
    #[test]
    fn saved_state_defaults_are_safe() {
        let state: Saved = serde_json::from_str("{}").unwrap();
        assert!(state.session.is_empty());
    }
}
