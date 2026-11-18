use crate::{Ctx, admin_api as api, provider_api};
use leptos::prelude::*;
use serde_json::{Value, json};

const PATH: &str = "/api/model-specifications";
const TOKEN_FIELDS: [(&str, &str); 4] = [
    ("context_window", "上下文窗口"),
    ("max_input_tokens", "最大输入"),
    ("input_usage_reference", "输入用量参考值"),
    ("output_usage_reference", "输出用量参考值"),
];
const KINDS: [(&str, &str); 10] = [
    ("unknown", "待分类"), ("chat", "对话与推理"), ("embeddings", "文本嵌入"),
    ("rerank", "重排"), ("image", "图像"), ("video", "视频"), ("audio", "音频"),
    ("asr", "语音识别"), ("tts", "语音合成"), ("tools", "工具"),
];
const UNITS: [(&str, &str); 13] = [
    ("count", "个"), ("token", "Token"), ("dimension", "维"), ("document", "篇"),
    ("second", "秒"), ("millisecond", "毫秒"), ("character", "字符"),
    ("byte", "字节"), ("image", "张"), ("video", "段"), ("pixel", "像素"),
    ("frame", "帧"), ("frame_per_second", "帧/秒"),
];

const QUANTITY_FIELDS: [(&str, &str); 7] = [
    ("second", "时长（秒）"),
    ("minute", "按分钟计费的时长（秒）"),
    ("hour", "按小时计费的时长（秒）"),
    ("character", "字符数"),
    ("byte", "字节数"),
    ("image", "图像张数"),
    ("video", "视频段数"),
];

fn quantity_applies(kind: &str, unit: &str) -> bool {
    match kind {
        "asr" | "audio" => unit == "second",
        "tts" => matches!(unit, "character" | "byte"),
        "image" => unit == "image",
        "video" => matches!(unit, "second" | "video"),
        "unknown" => !matches!(unit, "minute" | "hour"),
        _ => false,
    }
}

pub(super) fn quantity_milli_input(value: i64) -> String {
    let whole = value / 1000;
    let fraction = value % 1000;
    if fraction == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{fraction:03}").trim_end_matches('0').to_owned()
    }
}

fn quantity_milli(raw: &str, name: &str) -> Result<i64, String> {
    super::policy_editor::optional_limit(raw).ok().flatten()
        .filter(|value| *value > 0 && *value % 1000 == 0
            && raw.split_once('.').is_none_or(|(_, fraction)| fraction.len() <= 3))
        .map(|value| value / 1000)
        .ok_or_else(|| format!("{name}须大于 0，最多 3 位小数。"))
}

fn label<'a>(options: &[(&str, &'a str)], key: &str) -> &'a str {
    options.iter().find(|(id, _)| *id == key).map(|(_, label)| *label).unwrap_or("")
}

fn input_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        _ => String::new(),
    }
}

pub(super) fn summary(spec: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(kind) = spec["kind"].as_str().filter(|kind| *kind != "unknown") {
        parts.push(label(&KINDS, kind).to_string());
    }
    for (key, name) in TOKEN_FIELDS {
        if let Some(value) = spec[key].as_i64() {
            parts.push(format!("{name} {value} Token"));
        }
    }
    for (unit, name) in QUANTITY_FIELDS {
        if let Some(value) = spec["quantity_usage_references"][unit].as_i64() {
            parts.push(format!("{name} {}", quantity_milli_input(value)));
        }
    }
    if let Some(metrics) = spec["metrics"].as_object() {
        for (name, metric) in metrics {
            let unit = label(&UNITS, metric["unit"].as_str().unwrap_or(""));
            parts.push(format!("{name} {} {unit}", input_value(&metric["value"])));
        }
    }
    parts.join(" · ")
}

#[derive(Clone)]
struct MetricInput {
    name: String,
    value: String,
    unit: String,
}

#[derive(Clone)]
pub(super) struct ChannelSpecDraft {
    pub model_id: String,
    pub provider_id: i64,
    pub matched: Option<Value>,
}

#[component]
pub(super) fn ModelSpecifications(
    ctx: Ctx,
    #[prop(optional)] channel_editor: Option<RwSignal<Option<ChannelSpecDraft>>>,
    #[prop(optional)] on_saved: Option<Callback<()>>,
) -> impl IntoView {
    let embedded = channel_editor.is_some();
    let open = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let records = RwSignal::new(Vec::<Value>::new());
    let providers = RwSignal::new(Vec::<provider_api::Provider>::new());
    let query = RwSignal::new(String::new());
    let editing = RwSignal::new(false);
    let existing = RwSignal::new(false);
    let deleting = RwSignal::new(false);
    let draft = RwSignal::new(Value::Null);
    let metrics = RwSignal::new(Vec::<(usize, ArcRwSignal<MetricInput>)>::new());
    let next_metric = RwSignal::new(0usize);
    let error = RwSignal::new(String::new());
    let message = RwSignal::new(String::new());

    let load = Callback::new(move |()| {
        busy.set(true);
        error.set(String::new());
        leptos::task::spawn_local(async move {
            let (specifications, channels) = futures_util::join!(
                api::request("GET", PATH, None),
                provider_api::list(),
            );
            if busy.try_get_untracked().is_none() { return; }
            match (specifications, channels) {
                (Ok(rows), Ok(channels)) => {
                    records.set(api::rows(&rows, "specifications"));
                    providers.set(channels.providers);
                }
                (Err(e), _) | (_, Err(e)) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    });

    let begin = Callback::new(move |(record, is_existing): (Option<Value>, bool)| {
        existing.set(is_existing);
        deleting.set(false);
        error.set(String::new());
        message.set(String::new());
        let record = record.unwrap_or_else(|| json!({"kind":"chat"}));
        let inputs = record["metrics"].as_object().into_iter().flatten().enumerate().map(|(id, (name, metric))| {
            (id, ArcRwSignal::new(MetricInput {
                name: name.clone(),
                value: input_value(&metric["value"]),
                unit: input_value(&metric["unit"]),
            }))
        }).collect::<Vec<_>>();
        next_metric.set(inputs.len());
        metrics.set(inputs);
        draft.set(record);
        editing.set(true);
    });

    if let Some(editor) = channel_editor {
        Effect::new(move |_| {
            if let Some(ChannelSpecDraft { model_id, provider_id, matched }) = editor.get() {
                editor.set(None);
                let mut record = matched.unwrap_or_else(|| json!({"kind":"unknown"}));
                let is_existing = record["provider_id"].as_i64() == Some(provider_id)
                    && record["origin"] == "maintained";
                record["model_id"] = json!(model_id);
                record["provider_id"] = json!(provider_id);
                begin.run((Some(record), is_existing));
                open.set(true);
            }
        });
    }

    let after_save = Callback::new(move |text: &'static str| {
        deleting.set(false);
        editing.set(false);
        if embedded {
            open.set(false);
            ctx.say(text);
            if let Some(on_saved) = on_saved { on_saved.run(()); }
        } else {
            message.set(text.into());
            load.run(());
        }
    });

    let save = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let draft = draft.get_untracked();
        let model_id = input_value(&draft["model_id"]).trim().to_string();
        let scope = input_value(&draft["provider_id"]).parse::<i64>().ok();
        let prepare = || -> Result<Value, String> {
            if model_id.is_empty() { return Err("请填写上游模型名。".into()); }
            if !existing.get_untracked() && records.get_untracked().iter().any(|row|
                row["model_id"].as_str() == Some(&model_id) && row["provider_id"].as_i64() == scope
            ) {
                return Err("此渠道范围下已存在该型号，请从列表打开编辑。".into());
            }
            let mut body = json!({"model_id":model_id,"provider_id":scope,"kind":draft["kind"]});
            let source = input_value(&draft["source_url"]).trim().to_string();
            if !source.is_empty() { body["source_url"] = json!(source); }
            for (key, caption) in TOKEN_FIELDS {
                let field = &draft[key];
                if embedded && !existing.get_untracked() && field.is_number()
                    && matches!(key, "input_usage_reference" | "output_usage_reference")
                {
                    continue;
                }
                let raw = input_value(field);
                if !raw.trim().is_empty() {
                    let input_only = matches!(draft["kind"].as_str(), Some("embeddings" | "rerank"));
                    let zero_allowed = key == "output_usage_reference" && input_only;
                    let amount = api::integer(&raw, caption, !zero_allowed)?;
                    if zero_allowed && amount != 0 {
                        return Err("文本嵌入和重排的输出用量参考值须为 0。".into());
                    }
                    body[key] = json!(amount);
                }
            }
            let mut quantities = serde_json::Map::new();
            for (unit, name) in QUANTITY_FIELDS {
                let field = &draft["quantity_usage_references"][unit];
                if let Some(value) = field.as_i64() {
                    if !embedded || existing.get_untracked() {
                        quantities.insert(unit.into(), json!(value));
                    }
                } else if let Some(raw) = field.as_str().map(str::trim).filter(|raw| !raw.is_empty()) {
                    quantities.insert(unit.into(), json!(quantity_milli(raw, name)?));
                }
            }
            body["quantity_usage_references"] = Value::Object(quantities);
            let mut values = serde_json::Map::new();
            for (_, metric) in metrics.get_untracked() {
                let MetricInput { name, value, unit } = metric.get_untracked();
                let name = name.trim();
                if name.is_empty() { return Err("请填写扩展指标名称，或移除空指标。".into()); }
                let value = api::integer(&value, name, true)?;
                if values.insert(name.into(), json!({"value":value,"unit":unit})).is_some() {
                    return Err(format!("扩展指标名称重复：{name}"));
                }
            }
            body["metrics"] = Value::Object(values);
            Ok(body)
        };
        let body = match prepare() {
            Ok(body) => body,
            Err(reason) => { error.set(reason); return; }
        };
        busy.set(true);
        error.set(String::new());
        leptos::task::spawn_local(async move {
            let result = api::request("PUT", PATH, Some(body)).await;
            if busy.try_get_untracked().is_none() { return; }
            busy.set(false);
            match result {
                Ok(_) => after_save.run("模型规格已保存。"),
                Err(e) => { ctx.note_err(&e); error.set(e.message); }
            }
        });
    };

    let delete = move |_| {
        let row = draft.get_untracked();
        let model = api::encode(row["model_id"].as_str().unwrap_or(""));
        let mut path = format!("{PATH}?model_id={model}");
        if let Some(provider) = row["provider_id"].as_i64() {
            path.push_str(&format!("&provider_id={provider}"));
        }
        busy.set(true);
        error.set(String::new());
        leptos::task::spawn_local(async move {
            let result = api::request("DELETE", &path, None).await;
            if busy.try_get_untracked().is_none() { return; }
            busy.set(false);
            match result {
                Ok(_) => after_save.run("模型规格已删除。"),
                Err(e) => { ctx.note_err(&e); error.set(e.message); }
            }
        });
    };

    view! {
        {(!embedded).then(|| view! {<button type="button" class="ghost" aria-haspopup="dialog" on:click=move |_| {
            editing.set(false);
            query.set(String::new());
            message.set(String::new());
            open.set(true);
            load.run(());
        }>"模型规格"</button>})}
        <super::ModuleDialog open=open busy=busy title=if embedded { "编辑渠道规格".into() } else { "模型规格".into() }>
            <Show when=move || !embedded && !editing.get()>
                <label class="account-field">
                    <span>"搜索上游模型名"</span>
                    <input type="search" prop:value=query on:input=move |e| query.set(event_target_value(&e))/>
                </label>
                <div class="account-actions">
                    <button type="button" class="primary" disabled=busy on:click=move |_| begin.run((None, false))>"新增规格"</button>
                    <button type="button" class="ghost" disabled=busy on:click=move |_| load.run(())>"刷新"</button>
                </div>
                <Show when=move || busy.get()><p role="status">"正在读取模型规格…"</p></Show>
                <div class="table-wrap model-specifications-list">
                    <table>
                        <thead><tr><th scope="col">"上游模型名"</th><th scope="col">"适用渠道"</th><th scope="col">"规格"</th><th scope="col">"操作"</th></tr></thead>
                        <tbody>{move || {
                            let query = query.get().trim().to_lowercase();
                            let providers = providers.get();
                            let rows = records.get().into_iter().filter(|row| {
                                row["model_id"].as_str().is_some_and(|id| id.to_lowercase().contains(&query))
                            }).map(|row| {
                                let scope = match row["provider_id"].as_i64() {
                                    Some(id) => providers.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_else(|| format!("渠道 #{id}")),
                                    None => "全部渠道".into(),
                                };
                                let name = api::text(&row, "model_id");
                                let edit_label = format!("编辑 {name}（{scope}）");
                                let details = summary(&row);
                                view! {
                                    <tr><th scope="row">{name}</th><td>{scope}</td><td>{details}</td><td>
                                        <button type="button" class="ghost" aria-label=edit_label disabled=busy on:click=move |_| begin.run((Some(row.clone()), true))>"编辑"</button>
                                    </td></tr>
                                }
                            }).collect::<Vec<_>>();
                            if rows.is_empty() && !busy.get() {
                                return view! { <tr><td colspan="4">"暂无匹配的模型规格。"</td></tr> }.into_any();
                            }
                            rows.into_iter().collect_view().into_any()
                        }}</tbody>
                    </table>
                </div>
            </Show>
            <Show when=move || editing.get()>
                <form class="account-form model-specification-form" on:submit=save>
                    <fieldset disabled=busy>
                        <div class="policy-limits">
                            <label class="account-field"><span>"上游模型名"</span>
                                <input required maxlength="512" readonly=move || embedded || existing.get() prop:value=move || input_value(&draft.get()["model_id"])
                                    on:input=move |e| draft.update(|row| row["model_id"] = json!(event_target_value(&e))) />
                            </label>
                            {embedded.then(|| view! {<p class="account-help">"规格适用于本渠道的同名上游模型。只调整用量参考值时，该渠道后续定价会使用你保存的值。"</p>})}
                            <Show when=move || !embedded><label class="account-field"><span>"适用渠道"</span>
                                <select aria-label="适用渠道" disabled=existing prop:value=move || input_value(&draft.get()["provider_id"])
                                    on:change=move |e| draft.update(|row| row["provider_id"] = json!(event_target_value(&e).parse::<i64>().ok()))>
                                    <option value="">"全部渠道"</option>
                                    {move || providers.get().into_iter().map(|p| view! {
                                        <option value=p.id.to_string() selected=move || draft.get()["provider_id"].as_i64() == Some(p.id)>{p.name}</option>
                                    }).collect_view()}
                                </select>
                            </label></Show>
                            <label class="account-field"><span>"模型类型"</span>
                                <select aria-label="模型类型" prop:value=move || input_value(&draft.get()["kind"]) on:change=move |e| draft.update(|row| row["kind"] = json!(event_target_value(&e)))>
                                    {KINDS.into_iter().map(|(kind, name)| view! {
                                        <option value=kind selected=move || draft.get()["kind"] == kind>{name}</option>
                                    }).collect_view()}
                                </select>
                            </label>
                        </div>
                        <div class="policy-limits">
                            {TOKEN_FIELDS.into_iter().map(|(key, name)| view! {
                                <label class="account-field"><span>{format!("{name}（Token）")}</span>
                                    <input type="text" inputmode="numeric" placeholder="未知时留空"
                                        prop:value=move || input_value(&draft.get()[key])
                                        on:input=move |e| draft.update(|row| row[key] = json!(event_target_value(&e))) />
                                </label>
                            }).collect_view()}
                        </div>
                        <p class="account-help">"上下文窗口是输入与输出共用的容量，最大输入是单独公布的输入上限。输入和输出用量参考值会带入模型价格，用于费用估算。"</p>
                        <Show when=move || {
                            let record = draft.get();
                            QUANTITY_FIELDS.iter().any(|(unit, _)| quantity_applies(
                                record["kind"].as_str().unwrap_or("unknown"), unit
                            ) || !record["quantity_usage_references"][*unit].is_null())
                        }>
                            <h3>"数量用量参考值"</h3>
                            <p class="account-help">"填写单次调用的参考时长或数量。时长统一填写秒数，计费时按所选单位换算。"</p>
                            <div class="policy-limits">
                                {QUANTITY_FIELDS.into_iter().map(|(unit, name)| view! {
                                    <Show when=move || {
                                        let record = draft.get();
                                        quantity_applies(record["kind"].as_str().unwrap_or("unknown"), unit)
                                            || !record["quantity_usage_references"][unit].is_null()
                                    }>
                                        <label class="account-field"><span>{name}</span>
                                            <input type="text" inputmode="decimal" placeholder="未知时留空"
                                                prop:value=move || {
                                                    let row = draft.get();
                                                    let value = &row["quantity_usage_references"][unit];
                                                    value.as_i64().map(quantity_milli_input).unwrap_or_else(|| input_value(value))
                                                }
                                                on:input=move |e| draft.update(|row| {
                                                    row["quantity_usage_references"][unit] = json!(event_target_value(&e));
                                                })/>
                                        </label>
                                    </Show>
                                }).collect_view()}
                            </div>
                        </Show>
                        <label class="account-field"><span>"规格来源链接（选填）"</span>
                            <input type="url" maxlength="2048" prop:value=move || input_value(&draft.get()["source_url"])
                                on:input=move |e| draft.update(|row| row["source_url"] = json!(event_target_value(&e))) />
                        </label>
                        <h3>"扩展指标"</h3>
                        <p class="account-help">"可记录向量维度、重排文档数、音频时长、图像宽高或视频帧率。名称使用小写英文字母、数字和下划线，例如 embedding_dimensions。"</p>
                        <For each=move || metrics.get() key=|(id, _)| *id children=move |(id, metric)| {
                            let name = metric.clone();
                            let edit_name = metric.clone();
                            let value = metric.clone();
                            let edit_value = metric.clone();
                            let unit = metric.clone();
                            let selections = metric.clone();
                            view! {
                            <div class="policy-limits">
                                <label class="account-field"><span>"指标名称"</span>
                                    <input maxlength="64" prop:value=move || name.get().name on:input=move |e| edit_name.update(|row| row.name = event_target_value(&e)) />
                                </label>
                                <label class="account-field"><span>"数值"</span>
                                    <input type="text" inputmode="numeric" prop:value=move || value.get().value on:input=move |e| edit_value.update(|row| row.value = event_target_value(&e)) />
                                </label>
                                <label class="account-field"><span>"单位"</span>
                                    <select aria-label="单位" prop:value=move || unit.get().unit on:change=move |e| metric.update(|row| row.unit = event_target_value(&e))>
                                        {UNITS.into_iter().map(|(unit, name)| {
                                            let selection = selections.clone();
                                            view! {<option value=unit selected=move || selection.get().unit == unit>{name}</option>}
                                        }).collect_view()}
                                    </select>
                                </label>
                            </div>
                            <button type="button" class="ghost" on:click=move |_| metrics.update(|rows| rows.retain(|(row_id, _)| *row_id != id))>"移除此指标"</button>
                            }
                        }/>
                        <button type="button" class="ghost" on:click=move |_| {
                            let id = next_metric.get_untracked();
                            next_metric.set(id + 1);
                            metrics.update(|rows| rows.push((id, ArcRwSignal::new(MetricInput {name:String::new(),value:String::new(),unit:"count".into()}))));
                        }>"添加指标"</button>
                        <div class="account-actions">
                            <button type="submit" class="primary">"保存规格"</button>
                            <button type="button" class="ghost" on:click=move |_| {
                                editing.set(false); error.set(String::new());
                                if embedded { open.set(false); }
                            }>{if embedded { "取消" } else { "返回列表" }}</button>
                            <Show when=move || existing.get() && !deleting.get()>
                                <button type="button" class="ghost danger" on:click=move |_| deleting.set(true)>"删除规格"</button>
                            </Show>
                        </div>
                        <Show when=move || deleting.get()>
                            <div role="group" aria-label="确认删除模型规格">
                                <p role="alert">"确认删除此规格？之后会按可用的其他规格匹配。"</p>
                                <div class="account-actions">
                                    <button type="button" class="ghost danger" on:click=delete>"确认删除规格"</button>
                                    <button type="button" class="ghost" on:click=move |_| deleting.set(false)>"取消删除"</button>
                                </div>
                            </div>
                        </Show>
                    </fieldset>
                </form>
            </Show>
            <Show when=move || !error.get().is_empty()><p class="account-error" role="alert">{move || error.get()}</p></Show>
            <p role="status">{move || message.get()}</p>
        </super::ModuleDialog>
    }
}
