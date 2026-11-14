//! Shared cursor-list affordances. Observe the tail rather than scroll events;
//! only one page is requested at a time and errors require an explicit retry.
use leptos::prelude::*;
use wasm_bindgen::{closure::Closure, JsCast};

#[component]
pub fn ScrollFeed(
    busy: Signal<bool>, has_more: Signal<bool>, failed: Signal<bool>,
    on_more: Callback<()>, on_latest: Callback<()>,
    #[prop(default = "记录")] label: &'static str,
    #[prop(default = Signal::derive(|| true))] show_latest: Signal<bool>,
    #[prop(default = "")] anchor: &'static str,
) -> impl IntoView {
    let tail = NodeRef::<leptos::html::Div>::new();
    let visible = RwSignal::new(false);
    let supported = RwSignal::new(true);
    let observer = StoredValue::new_local(None::<(web_sys::IntersectionObserver, Closure<dyn FnMut(js_sys::Array, web_sys::IntersectionObserver)>)>);
    Effect::new(move |_| {
        let Some(element) = tail.get() else { return; };
        if observer.with_value(|v| v.is_some()) { return; }
        let callback = Closure::wrap(Box::new(move |entries: js_sys::Array, _: web_sys::IntersectionObserver| {
            for entry in entries.iter() {
                if let Ok(entry) = entry.dyn_into::<web_sys::IntersectionObserverEntry>() {
                    let _ = visible.try_set(entry.is_intersecting());
                }
            }
        }) as Box<dyn FnMut(js_sys::Array, web_sys::IntersectionObserver)>);
        let options = web_sys::IntersectionObserverInit::new();
        options.set_root_margin("0px 0px 240px 0px");
        match web_sys::IntersectionObserver::new_with_options(callback.as_ref().unchecked_ref(), &options) {
            Ok(value) => { value.observe(&element); observer.set_value(Some((value, callback))); }
            Err(_) => supported.set(false),
        }
    });
    // A completed append can move the tail out of view before the browser has
    // delivered its next observation. Re-observe after layout instead of using
    // the previous batch's visibility to issue another request immediately.
    Effect::new(move |_| {
        if busy.get() { return; }
        if let Some(element) = tail.get() {
            observer.with_value(|value| {
                if let Some((observer, _)) = value {
                    observer.unobserve(&element);
                    observer.observe(&element);
                }
            });
        }
    });
    Effect::new(move |_| {
        if visible.get() && !busy.get() && has_more.get() && !failed.get() {
            visible.set(false);
            untrack(move || on_more.run(()));
        }
    });
    on_cleanup(move || observer.update_value(|value| {
        if let Some((observer, _callback)) = value.take() { observer.take_records(); observer.disconnect(); }
    }));
    let latest = move |_| {
        visible.set(false);
        on_latest.run(());
        if let Some(window) = web_sys::window() {
            if let Some(element) = window.document().and_then(|d| d.get_element_by_id(anchor)) {
                element.scroll_into_view();
            } else { window.scroll_to_with_x_and_y(0.0, 0.0); }
        }
    };
    view! { <div class="scroll-feed">
        <div class="scroll-feed-sentinel" node_ref=tail aria-live="polite" role="status">
            <span>{move || if busy.get() {format!("正在加载{label}…")} else if failed.get() {"加载失败，请重试".into()} else if !has_more.get() {format!("已加载全部{label}")} else {String::new()}}</span>
            <Show when=move || !busy.get() && (failed.get() || (!supported.get() && has_more.get()))>
                <button type="button" class="ghost scroll-feed-icon" aria-label="重新加载" title="重新加载" on:click=move |_| {visible.set(false);on_more.run(());}>
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><path d="M20 7v5h-5M4 17v-5h5M6.1 7a7 7 0 0 1 11.6-1L20 9M4 15l2.3 3A7 7 0 0 0 18 17"/></svg>
                </button>
            </Show>
        </div>
        <Show when=move || show_latest.get()>
            <button type="button" class="ghost scroll-feed-icon scroll-feed-latest" aria-label="回到最新" title="回到最新" disabled=move || busy.get() on:click=latest>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><path d="M5 4h14M6 13l6-6 6 6M12 7v14"/></svg>
            </button>
        </Show>
    </div> }
}
