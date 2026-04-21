//! Yew components for the popup UI.
//!
//! [`App`] is the root component. It delegates to [`LockView`] when no session
//! is active and [`UnlockedView`] once a user has authenticated.

use std::collections::HashMap;
use std::str::FromStr;
use stylist::yew::{Global, styled_component};
use valet::Record;
use valet::lot::DEFAULT_LOT;
use valet::password::Password;
use valet::record::Label;
use valet::uuid::Uuid;
use valetd::Request;
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

use super::browser;
use crate::rpc;

/// How long (ms) before the clipboard is cleared after copying a password.
const CLIPBOARD_CLEAR_MS: u32 = 20_000;

/// Active user session state carried between popup components.
#[derive(Clone, PartialEq)]
pub(crate) struct Session {
    pub(crate) username: String,
    pub(crate) lot: String,
    pub(crate) domain: Option<String>,
}

/// Feedback message shown at the bottom of the popup.
#[derive(Clone, PartialEq)]
pub(crate) enum Message {
    None,
    Status(String),
    Error(String),
}

impl Message {
    fn class(&self) -> &'static str {
        match self {
            Message::Error(_) => "err",
            _ => "ok",
        }
    }
    fn text(&self) -> &str {
        match self {
            Message::None => "",
            Message::Status(s) | Message::Error(s) => s,
        }
    }
}

/// Root popup component. Shows [`LockView`] or [`UnlockedView`] depending
/// on whether a session is active.
#[styled_component(App)]
pub fn app() -> Html {
    let session = use_state(|| None::<Session>);
    let message = use_state(|| Message::None);
    let booted = use_state(|| false);
    let needs_permissions = use_state(|| false);

    {
        let session = session.clone();
        let booted = booted.clone();
        let needs_permissions = needs_permissions.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                // Check host permissions for autofill.
                if !browser::has_host_permissions().await
                    && !browser::permissions_banner_dismissed().await
                {
                    tracing::info!("host permissions not granted");
                    needs_permissions.set(true);
                }

                let result: Result<Vec<String>, valetd::request::Error<rpc::Error>> =
                    async { Ok(rpc::call(Request::Status).await?.expect_users()?) }.await;
                match result {
                    Ok(unlocked) => {
                        tracing::debug!(count = unlocked.len(), "status check ok");
                        if let Some(username) = unlocked.into_iter().next() {
                            let domain = browser::current_tab_domain().await;
                            session.set(Some(Session {
                                username,
                                lot: DEFAULT_LOT.to_string(),
                                domain,
                            }));
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "status check failed"),
                }
                booted.set(true);
            });
            || ()
        });
    }

    let global_css = css!(
        r#"
        body {
            font: 13px system-ui, sans-serif;
            width: 320px;
            margin: 0;
            padding: 12px;
        }
        h2 {
            font-size: 14px;
            margin: 0 0 8px;
        }
        label {
            display: block;
            font-size: 11px;
            color: #555;
            margin-top: 8px;
        }
        input, select, button {
            width: 100%;
            box-sizing: border-box;
            padding: 6px;
            margin-top: 2px;
        }
        hr {
            border: none;
            border-top: 1px solid #eee;
            margin: 12px 0;
        }
        .err {
            color: #b00020;
            margin-top: 6px;
            min-height: 1em;
        }
        .ok {
            color: #006622;
            margin-top: 6px;
            min-height: 1em;
        }
        "#
    );

    let on_unlock = {
        let session = session.clone();
        Callback::from(move |s: Session| session.set(Some(s)))
    };
    let on_lock = {
        let session = session.clone();
        let message = message.clone();
        Callback::from(move |_| {
            let session = session.clone();
            let message = message.clone();
            spawn_local(async move {
                let result: Result<(), valetd::request::Error<rpc::Error>> =
                    async { Ok(rpc::call(Request::LockAll).await?.expect_ok()?) }.await;
                match result {
                    Ok(()) => tracing::debug!("locked all users"),
                    Err(e) => tracing::warn!(error = %e, "lock_all failed"),
                }
                session.set(None);
                message.set(Message::Status("Locked.".into()));
            });
        })
    };
    let set_message = {
        let message = message.clone();
        Callback::from(move |m: Message| message.set(m))
    };
    let grant_permissions = {
        let needs_permissions = needs_permissions.clone();
        Callback::from(move |_| {
            let needs_permissions = needs_permissions.clone();
            spawn_local(async move {
                if browser::request_host_permissions().await {
                    tracing::info!("host permissions granted");
                    needs_permissions.set(false);
                } else {
                    tracing::warn!("host permissions denied");
                }
            });
        })
    };
    let dismiss_permissions = {
        let needs_permissions = needs_permissions.clone();
        Callback::from(move |_| {
            let needs_permissions = needs_permissions.clone();
            spawn_local(async move {
                browser::dismiss_permissions_banner().await;
                needs_permissions.set(false);
            });
        })
    };

    if !*booted {
        return html! { <><Global css={global_css} /><h2>{"Valet"}</h2></> };
    }

    html! {
        <>
            <Global css={global_css} />
            <h2>{"Valet"}</h2>
            { if *needs_permissions {
                html! {
                    <div style="margin-bottom:8px;padding:8px;background:#fff3cd;border-radius:4px;font-size:12px;">
                        {"Valet needs permission to autofill on websites."}
                        <button onclick={grant_permissions} style="margin-top:6px;">
                            {"Grant permissions"}
                        </button>
                        <div style="margin-top:4px;text-align:right;">
                            <a href="#" onclick={dismiss_permissions} style="font-size:11px;color:#666;">
                                {"dismiss"}
                            </a>
                        </div>
                    </div>
                }
            } else {
                html! {}
            } }
            { match (*session).clone() {
                None => html! { <LockView {on_unlock} set_message={set_message.clone()} /> },
                Some(s) => html! { <UnlockedView session={s} {on_lock} set_message={set_message.clone()} /> },
            } }
            <div class={classes!(message.class())}>{ message.text().to_string() }</div>
        </>
    }
}

#[derive(Properties, PartialEq)]
struct LockProps {
    on_unlock: Callback<Session>,
    set_message: Callback<Message>,
}

#[styled_component(LockView)]
fn lock_view(props: &LockProps) -> Html {
    let users = use_state(Vec::<String>::new);
    let username = use_state(String::new);
    let password = use_state(String::new);

    {
        let users = users.clone();
        let username = username.clone();
        let set_message = props.set_message.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                let result: Result<Vec<String>, valetd::request::Error<rpc::Error>> =
                    async { Ok(rpc::call(Request::ListUsers).await?.expect_users()?) }.await;
                match result {
                    Ok(list) => {
                        if list.is_empty() {
                            set_message.emit(Message::Error(
                                "No users registered. Run `valet register` from the CLI first."
                                    .into(),
                            ));
                        } else {
                            if username.is_empty() {
                                username.set(list[0].clone());
                            }
                            users.set(list);
                        }
                    }
                    Err(e) => {
                        set_message.emit(Message::Error(format!("Failed to list users: {e}")))
                    }
                }
            });
            || ()
        });
    }

    let on_user_change = {
        let username = username.clone();
        Callback::from(move |e: Event| {
            let target: HtmlSelectElement = e.target_unchecked_into();
            username.set(target.value());
        })
    };
    let on_password_input = {
        let password = password.clone();
        Callback::from(move |e: InputEvent| {
            let target: HtmlInputElement = e.target_unchecked_into();
            password.set(target.value());
        })
    };
    let do_unlock = {
        let username = username.clone();
        let password = password.clone();
        let on_unlock = props.on_unlock.clone();
        let set_message = props.set_message.clone();
        Callback::from(move |_| {
            let u = (*username).clone();
            let p = (*password).clone();
            if u.is_empty() || p.is_empty() {
                set_message.emit(Message::Error("username and password required".into()));
                return;
            }
            let on_unlock = on_unlock.clone();
            let set_message = set_message.clone();
            let password_state = password.clone();
            spawn_local(async move {
                let password: Password = match p.as_str().try_into() {
                    Ok(pw) => pw,
                    Err(_) => {
                        set_message.emit(Message::Error("password too long".into()));
                        return;
                    }
                };
                let result: Result<(), valetd::request::Error<rpc::Error>> = async {
                    Ok(rpc::call(Request::Unlock {
                        username: u.clone(),
                        password,
                    })
                    .await?
                    .expect_ok()?)
                }
                .await;
                match result {
                    Ok(()) => {
                        let domain = browser::current_tab_domain().await;
                        tracing::debug!(username = %u, domain = ?domain, "unlock succeeded");
                        password_state.set(String::new());
                        set_message.emit(Message::None);
                        on_unlock.emit(Session {
                            username: u,
                            lot: DEFAULT_LOT.to_string(),
                            domain,
                        });
                    }
                    Err(e) => {
                        tracing::debug!(username = %u, error = %e, "unlock failed");
                        set_message.emit(Message::Error(format!("Unlock failed: {e}")));
                    }
                }
            });
        })
    };
    let on_password_keydown = {
        let do_unlock = do_unlock.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" {
                do_unlock.emit(());
            }
        })
    };

    html! {
        <section>
            <label for="user-select">{"User"}</label>
            <select id="user-select" onchange={on_user_change} value={(*username).clone()}>
                { for users.iter().map(|u| html! {
                    <option value={u.clone()} selected={*u == *username}>{ u }</option>
                }) }
            </select>
            <label for="password-input">{"Password"}</label>
            <PasswordInput
                id="password-input"
                value={(*password).clone()}
                oninput={on_password_input}
                onkeydown={on_password_keydown}
            />
            <button onclick={Callback::from(move |_| do_unlock.emit(()))}>{"Unlock"}</button>
        </section>
    }
}

#[derive(Properties, PartialEq)]
struct UnlockedProps {
    session: Session,
    on_lock: Callback<()>,
    set_message: Callback<Message>,
}

#[styled_component(UnlockedView)]
fn unlocked_view(props: &UnlockedProps) -> Html {
    let records = use_state(Vec::<(Uuid<Record>, Label)>::new);
    let new_id = use_state(String::new);
    let new_password = use_state(String::new);
    let refresh_tick = use_state(|| 0u32);

    let style = css!(
        r#"
        .row {
            display: flex;
            gap: 4px;
            margin-top: 6px;
        }
        .row > * {
            flex: 1;
        }
        .record {
            padding: 6px;
            border: 1px solid #ddd;
            border-radius: 4px;
            margin-top: 6px;
            display: flex;
            gap: 6px;
            justify-content: space-between;
            align-items: center;
        }
        .record .label {
            font-family: ui-monospace, monospace;
            font-size: 12px;
            min-width: 0;
            overflow: hidden;
            text-overflow: ellipsis;
            white-space: nowrap;
            flex: 1 1 auto;
        }
        .record button {
            width: auto;
            flex: 0 0 auto;
            margin-top: 0;
        }
        em {
            color: #666;
        }
        "#
    );

    {
        let records = records.clone();
        let session = props.session.clone();
        let set_message = props.set_message.clone();
        let tick = *refresh_tick;
        use_effect_with((session.clone(), tick), move |_| {
            match session.domain.clone() {
                None => records.set(Vec::new()),
                Some(domain) => spawn_local(async move {
                    let result: Result<Vec<(Uuid<Record>, Label)>, valetd::request::Error<rpc::Error>> = async {
                        Ok(rpc::call(Request::FindRecords {
                            username: session.username.clone(),
                            lot: session.lot.clone(),
                            query: domain.clone(),
                        })
                        .await?
                        .expect_index()?)
                    }
                    .await;
                    match result {
                        Ok(list) => {
                            tracing::debug!(domain = %domain, count = list.len(), "records loaded");
                            records.set(list);
                        }
                        Err(e) => {
                            tracing::debug!(domain = %domain, error = %e, "find_records failed");
                            set_message
                                .emit(Message::Error(format!("Failed to load records: {e}")));
                        }
                    }
                }),
            }
            || ()
        });
    }

    let copy = {
        let session = props.session.clone();
        let set_message = props.set_message.clone();
        Callback::from(move |uuid: String| {
            let session = session.clone();
            let set_message = set_message.clone();
            spawn_local(async move {
                let parsed_uuid: Uuid<Record> = match Uuid::parse(&uuid) {
                    Ok(u) => u,
                    Err(e) => {
                        set_message.emit(Message::Error(format!("Invalid uuid: {e:?}")));
                        return;
                    }
                };
                let result: Result<Record, valetd::request::Error<rpc::Error>> = async {
                    Ok(rpc::call(Request::GetRecord {
                        username: session.username.clone(),
                        lot: session.lot.clone(),
                        uuid: parsed_uuid,
                    })
                    .await?
                    .expect_record()?)
                }
                .await;
                match result {
                    Ok(record) => {
                        if let Err(e) = browser::copy_to_clipboard(record.password().as_str()).await
                        {
                            tracing::debug!(error = ?e, "clipboard write failed");
                            set_message.emit(Message::Error(format!("Copy failed: {e:?}")));
                            return;
                        }
                        tracing::debug!(uuid = %uuid, "password copied to clipboard");
                        set_message
                            .emit(Message::Status("Password copied. Clearing in 20s…".into()));
                        let set_message = set_message.clone();
                        spawn_local(async move {
                            gloo_timers::future::TimeoutFuture::new(CLIPBOARD_CLEAR_MS).await;
                            let _ = browser::copy_to_clipboard("").await;
                            set_message.emit(Message::Status("Clipboard cleared.".into()));
                        });
                    }
                    Err(e) => {
                        tracing::debug!(uuid = %uuid, error = %e, "get_record failed");
                        set_message.emit(Message::Error(format!("Copy failed: {e}")));
                    }
                }
            });
        })
    };

    let on_id_input = {
        let new_id = new_id.clone();
        Callback::from(move |e: InputEvent| {
            let target: HtmlInputElement = e.target_unchecked_into();
            new_id.set(target.value());
        })
    };
    let on_pw_input = {
        let new_password = new_password.clone();
        Callback::from(move |e: InputEvent| {
            let target: HtmlInputElement = e.target_unchecked_into();
            new_password.set(target.value());
        })
    };

    let save = {
        let session = props.session.clone();
        let set_message = props.set_message.clone();
        let new_id = new_id.clone();
        let new_password = new_password.clone();
        let refresh_tick = refresh_tick.clone();
        Callback::from(move |_| {
            let Some(domain) = session.domain.clone() else {
                set_message.emit(Message::Error(
                    "Need an active tab with a domain to save against.".into(),
                ));
                return;
            };
            let id = (*new_id).trim().to_string();
            let pw = (*new_password).clone();
            if id.is_empty() || pw.is_empty() {
                set_message.emit(Message::Error("identifier and password required".into()));
                return;
            }
            if !crate::password_is_valid(&pw) {
                set_message.emit(Message::Error("Password too short (valet minimum).".into()));
                return;
            }
            let session = session.clone();
            let set_message = set_message.clone();
            let new_id = new_id.clone();
            let new_password = new_password.clone();
            let refresh_tick = refresh_tick.clone();
            spawn_local(async move {
                let label = format!("{id}@{domain}");
                let parsed_label = match Label::from_str(&label) {
                    Ok(l) => l,
                    Err(e) => {
                        set_message.emit(Message::Error(format!("Invalid label: {e:?}")));
                        return;
                    }
                };
                let password: Password = match pw.as_str().try_into() {
                    Ok(p) => p,
                    Err(_) => {
                        set_message.emit(Message::Error("password too long".into()));
                        return;
                    }
                };
                let result: Result<Record, valetd::request::Error<rpc::Error>> = async {
                    Ok(rpc::call(Request::CreateRecord {
                        username: session.username.clone(),
                        lot: session.lot.clone(),
                        label: parsed_label,
                        password,
                        extra: HashMap::<String, String>::new(),
                    })
                    .await?
                    .expect_record()?)
                }
                .await;
                match result {
                    Ok(_record) => {
                        tracing::debug!(label = %label, "record saved");
                        new_id.set(String::new());
                        new_password.set(String::new());
                        set_message.emit(Message::Status(format!("Saved {label}.")));
                        refresh_tick.set(*refresh_tick + 1);
                    }
                    Err(e) => {
                        tracing::debug!(label = %label, error = %e, "save failed");
                        set_message.emit(Message::Error(format!("Save failed: {e}")));
                    }
                }
            });
        })
    };

    let domain_text = props
        .session
        .domain
        .clone()
        .unwrap_or_else(|| "(no domain)".to_string());
    let on_lock = props.on_lock.clone();

    html! {
        <section class={style}>
            <div class="row">
                <div>{"Unlocked: "}<strong>{ &props.session.username }</strong></div>
                <button onclick={Callback::from(move |_| on_lock.emit(()))}>{"Lock"}</button>
            </div>
            <hr />
            <div>{"Records for "}<strong>{ domain_text }</strong></div>
            <div>
                { if props.session.domain.is_none() {
                    html! { <em>{"No tab domain."}</em> }
                } else if records.is_empty() {
                    html! { <em>{"No records for this domain."}</em> }
                } else {
                    html! { for records.iter().map(|(uuid, label)| {
                        let copy = copy.clone();
                        let uuid = uuid.to_string();
                        html! {
                            <div class="record">
                                <span class="label">{ label.to_string() }</span>
                                <button onclick={Callback::from(move |_| copy.emit(uuid.clone()))}>{"Copy"}</button>
                            </div>
                        }
                    }) }
                } }
            </div>
            <hr />
            <h2>{"Save new credential"}</h2>
            <label for="new-id">{"Identifier (e.g. user)"}</label>
            <input id="new-id" type="text" value={(*new_id).clone()} oninput={on_id_input} />
            <label for="new-password">{"Password"}</label>
            <PasswordInput
                id="new-password"
                value={(*new_password).clone()}
                oninput={on_pw_input}
            />
            <button onclick={save}>{"Save for this domain"}</button>
        </section>
    }
}

/// Properties for [`PasswordInput`].
#[derive(Properties, PartialEq)]
struct PasswordInputProps {
    pub id: &'static str,
    pub value: String,
    pub oninput: Callback<InputEvent>,
    #[prop_or_default]
    pub onkeydown: Option<Callback<KeyboardEvent>>,
}

/// A password input the browser won't autofill.
///
/// Uses `type="text"` with CSS disc-masking so the browser never
/// recognises it as a password field. Clipboard paste works normally;
/// the value is stored as plain text in Yew state.
#[styled_component(PasswordInput)]
fn password_input(props: &PasswordInputProps) -> Html {
    let style = css!(
        r#"
        -webkit-text-security: disc;
        -moz-text-security: disc;
        text-security: disc;
        "#
    );

    html! {
        <input
            id={props.id}
            type="text"
            autocomplete="off"
            class={style}
            value={props.value.clone()}
            oninput={props.oninput.clone()}
            onkeydown={props.onkeydown.clone()}
        />
    }
}
