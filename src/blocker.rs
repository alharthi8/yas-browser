use adblock::{request::Request, Engine};
use std::{
    cell::{Cell, RefCell},
    fs,
    path::Path,
    rc::Rc,
};
use webview2_com::{
    take_pwstr, Microsoft::Web::WebView2::Win32::*, WebResourceRequestedEventHandler,
};
use windows::core::{Interface, HSTRING, PWSTR};
use wry::{WebView, WebViewExtWindows};

pub const SOURCES: [(&str, &str); 2] = [
    ("easylist.txt", "https://easylist.to/easylist/easylist.txt"),
    (
        "easyprivacy.txt",
        "https://easylist.to/easylist/easyprivacy.txt",
    ),
];
const FALLBACK: &str = "||doubleclick.net^\n||googlesyndication.com^\n||google-analytics.com^\n||googletagmanager.com^\n||adnxs.com^\n||connect.facebook.net^$third-party\n||adsystem.com^\n";
pub struct Shield {
    pub engine: Engine,
    pub exceptions: Vec<String>,
    pub count: usize,
    pub rules: usize,
}
impl Shield {
    pub fn load(root: &Path, custom: &str, exceptions: Vec<String>) -> Self {
        let mut lists = String::from(FALLBACK);
        for (file, _) in SOURCES {
            let bundled = if file == "easylist.txt" {
                include_str!("../filters/easylist.txt")
            } else {
                include_str!("../filters/easyprivacy.txt")
            };
            {
                let s = fs::read_to_string(root.join(file)).unwrap_or_else(|_| bundled.to_string());
                lists.push_str(&s);
                lists.push('\n');
            }
        }
        lists.push_str(custom);
        let rules = lists
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('!') && !l.starts_with('['))
            .count();
        Self {
            engine: Engine::new_with_list_text(lists),
            exceptions,
            count: 0,
            rules,
        }
    }
    pub fn enabled(&self, source: &str) -> bool {
        url::Url::parse(source)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .is_some_and(|h| !self.exceptions.contains(&h))
    }
}
pub fn attach(
    view: &WebView,
    shield: Rc<RefCell<Shield>>,
    source: Rc<RefCell<String>>,
    count: Rc<Cell<usize>>,
) -> windows::core::Result<()> {
    unsafe {
        let web = view.webview();
        let env = view.environment();
        if let Ok(v) = web.cast::<ICoreWebView2_22>() {
            v.AddWebResourceRequestedFilterWithRequestSourceKinds(
                &HSTRING::from("*"),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
            )?;
        } else {
            web.AddWebResourceRequestedFilter(
                &HSTRING::from("*"),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
            )?;
        }
        let mut token = 0;
        web.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let source = source.borrow();
                let mut shield = shield.borrow_mut();
                if !shield.enabled(&source) {
                    return Ok(());
                }
                let request = args.Request()?;
                let mut raw = PWSTR::null();
                request.Uri(&mut raw)?;
                let uri = take_pwstr(raw);
                let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
                args.ResourceContext(&mut context)?;
                let kind = match context {
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT => {
                        if uri == *source {
                            "document"
                        } else {
                            "subdocument"
                        }
                    }
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT => "script",
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE => "image",
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET => "stylesheet",
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT => "font",
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA => "media",
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST
                    | COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH => "xmlhttprequest",
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET => "websocket",
                    _ => "other",
                };
                let mut method = PWSTR::null();
                request.Method(&mut method)?;
                let method = take_pwstr(method);
                if let Ok(req) = Request::new(&uri, &source, kind, &method) {
                    let result = shield.engine.check_network_request(&req);
                    if result.should_block() {
                        let response = env.CreateWebResourceResponse(
                            None,
                            403,
                            &HSTRING::from("Blocked by Yas Shield"),
                            &HSTRING::from("Content-Type: text/plain\r\nCache-Control: no-store"),
                        )?;
                        args.SetResponse(&response)?;
                        count.set(count.get() + 1);
                        shield.count += 1;
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}
pub fn cosmetic(view: &WebView, shield: Rc<RefCell<Shield>>, url: String) {
    if !shield.borrow().enabled(&url) {
        return;
    }
    let resource = shield.borrow().engine.url_cosmetic_resources(&url);
    let selectors: Vec<_> = resource.hide_selectors.into_iter().collect();
    let css = selectors
        .into_iter()
        .map(|s| format!("{s}{{display:none!important}}\n"))
        .collect::<String>();
    let script=format!("(()=>{{if(location.href!=={})return;let s=document.getElementById('yas-shield-css');if(!s){{s=document.createElement('style');s.id='yas-shield-css';(document.head||document.documentElement).appendChild(s)}}s.textContent={};}})()",serde_json::to_string(&url).unwrap(),serde_json::to_string(&css).unwrap());
    let _ = view.evaluate_script(&script);
    // Generic selectors are resolved from the actual page classes and IDs, honoring exceptions.
    if !resource.generichide {
        let web = view.webview();
        let callback_view = web.clone();
        unsafe {
            let _=web.ExecuteScript(&HSTRING::from("JSON.stringify({classes:[...new Set([...document.querySelectorAll('[class]')].flatMap(e=>[...e.classList]))].slice(0,20000),ids:[...document.querySelectorAll('[id]')].map(e=>e.id).slice(0,20000)})"), &webview2_com::ExecuteScriptCompletedHandler::create(Box::new(move |hr,result| {
            if hr.is_err() { return Ok(()); }
            let raw=serde_json::from_str::<String>(&result).unwrap_or_default();
            if let Ok(v)=serde_json::from_str::<serde_json::Value>(&raw) {
                let classes:Vec<_>=v["classes"].as_array().into_iter().flatten().filter_map(|s|s.as_str()).collect();
                let ids:Vec<_>=v["ids"].as_array().into_iter().flatten().filter_map(|s|s.as_str()).collect();
                let selectors=shield.borrow().engine.hidden_class_id_selectors(classes,ids,&resource.exceptions);
                let css=selectors.into_iter().map(|s|format!("{s}{{display:none!important}}\n")).collect::<String>();
                let script=format!("if(location.href==={}){{let s=document.createElement('style');s.textContent={};(document.head||document.documentElement).appendChild(s)}}",serde_json::to_string(&url).unwrap(),serde_json::to_string(&css).unwrap());
                let _=callback_view.ExecuteScript(&HSTRING::from(script),None);
            }
            Ok(())
        })));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blocks_and_honors_exception_rules() {
        let engine = Engine::new_with_list_text(
            "||ads.example^\n@@||ads.example/allowed.js$script\n".to_string(),
        );
        assert!(engine
            .check_network_request(
                &Request::new(
                    "https://ads.example/ad.js",
                    "https://news.example",
                    "script",
                    "GET"
                )
                .unwrap()
            )
            .should_block());
        assert!(!engine
            .check_network_request(
                &Request::new(
                    "https://ads.example/allowed.js",
                    "https://news.example",
                    "script",
                    "GET"
                )
                .unwrap()
            )
            .should_block());
        assert!(!engine
            .check_network_request(
                &Request::new(
                    "https://news.example/main.js",
                    "https://news.example",
                    "script",
                    "GET"
                )
                .unwrap()
            )
            .should_block());
    }
}
