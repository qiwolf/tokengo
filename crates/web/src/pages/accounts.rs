//! Compatibility mount: old accounts bookmarks open the unified user workflow.
use leptos::prelude::*;
#[component]
pub fn Accounts(ctx: crate::Ctx) -> impl IntoView {
    view! {<super::users::Users ctx=ctx/>}
}
