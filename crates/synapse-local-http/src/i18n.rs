//! Display-language selection and the compile-time-checked UI message catalog.
//!
//! Only application-supplied interface text is localized. User data (Subject,
//! creator names, notes, rationales, pins, public text, stored history), API
//! identifiers, error codes, and problem JSON are never translated.
//!
//! One rule resolves the page language for HTML routes, in priority order:
//!
//! 1. an explicit `?lang=ja` or `?lang=en` on a page `GET`/`HEAD` is stored in
//!    the [`LANGUAGE_COOKIE`] and answered with `303 See Other` to the same path
//!    without `lang` (other query parameters are preserved);
//! 2. a supported [`LANGUAGE_COOKIE`] value;
//! 3. the highest-weighted supported primary subtag in `Accept-Language`;
//! 4. Japanese.
//!
//! Unsupported or conflicting `lang` values are ignored without an error page.
//! The browser script reads the server-resolved `<html lang>`, so templates and
//! client messages always follow this same rule.

use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::{
    ACCEPT_LANGUAGE, CONTENT_LANGUAGE, CONTENT_TYPE, COOKIE, LOCATION, SET_COOKIE, VARY,
};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::borrow::Borrow;
use std::convert::Infallible;

use crate::security::SecurityPolicy;

/// Cookie that stores only the explicit display-language preference.
///
/// Browsers do not scope cookies by port, so every `synapse-local` process on
/// the same loopback host shares this preference. It grants no authority.
pub(crate) const LANGUAGE_COOKIE: &str = "synapse_local_lang";
const LANGUAGE_COOKIE_ATTRIBUTES: &str = "Path=/; Max-Age=31536000; SameSite=Strict; HttpOnly";
const LANGUAGE_QUERY_PARAMETER: &str = "lang";
const PAGE_VARY: &str = "Accept-Language, Cookie";

/// A supported interface language.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Locale {
    Ja,
    En,
}

impl Locale {
    /// The final fallback keeps the historical Japanese interface.
    pub(crate) const DEFAULT: Self = Self::Ja;
    #[cfg(test)]
    pub(crate) const ALL: [Self; 2] = [Self::Ja, Self::En];

    /// The BCP 47 tag used for `<html lang>`, `Content-Language`, and the cookie.
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Ja => "ja",
            Self::En => "en",
        }
    }

    pub(crate) const fn is_ja(self) -> bool {
        matches!(self, Self::Ja)
    }

    pub(crate) const fn is_en(self) -> bool {
        matches!(self, Self::En)
    }

    /// Parse an exact supported code, ignoring ASCII case only.
    pub(crate) fn from_code(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("ja") {
            Some(Self::Ja)
        } else if value.eq_ignore_ascii_case("en") {
            Some(Self::En)
        } else {
            None
        }
    }

    pub(crate) const fn messages(self) -> &'static Messages {
        match self {
            Self::Ja => &JA,
            Self::En => &EN,
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Locale {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<Locale>()
            .copied()
            .unwrap_or_else(|| resolve_locale(&parts.headers)))
    }
}

/// Resolve the stored or negotiated language for a request without a valid
/// explicit `lang` parameter.
pub(crate) fn resolve_locale(headers: &HeaderMap) -> Locale {
    cookie_locale(headers)
        .or_else(|| accept_language_locale(headers))
        .unwrap_or(Locale::DEFAULT)
}

fn cookie_locale(headers: &HeaderMap) -> Option<Locale> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .filter(|(name, _)| *name == LANGUAGE_COOKIE)
        .find_map(|(_, value)| Locale::from_code(value.trim()))
}

fn accept_language_locale(headers: &HeaderMap) -> Option<Locale> {
    let mut best: Option<(u16, Locale)> = None;
    for item in headers
        .get_all(ACCEPT_LANGUAGE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
    {
        let mut parts = item.split(';');
        let tag = parts.next().unwrap_or_default().trim();
        let primary = tag.split('-').next().unwrap_or_default();
        let Some(locale) = Locale::from_code(primary) else {
            continue;
        };
        let mut weight = Some(1000);
        for parameter in parts {
            let parameter = parameter.trim();
            if let Some((name, value)) = parameter.split_once('=') {
                if name.trim().eq_ignore_ascii_case("q") {
                    weight = parse_quality(value.trim());
                }
            }
        }
        // q=0 means "not acceptable"; an invalid weight is ignored the same way.
        let Some(weight) = weight.filter(|weight| *weight > 0) else {
            continue;
        };
        // Strictly greater keeps the earliest entry among equal weights.
        if best.is_none_or(|(current, _)| weight > current) {
            best = Some((weight, locale));
        }
    }
    best.map(|(_, locale)| locale)
}

/// Parse an RFC 9110 `qvalue` into thousandths.
fn parse_quality(value: &str) -> Option<u16> {
    let (integer, fraction) = value.split_once('.').unwrap_or((value, ""));
    if fraction.len() > 3 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let mut thousandths = 0_u16;
    for (index, digit) in fraction.bytes().enumerate() {
        thousandths += u16::from(digit - b'0') * [100, 10, 1][index];
    }
    match integer {
        "0" => Some(thousandths),
        "1" if thousandths == 0 => Some(1000),
        _ => None,
    }
}

/// Return the explicitly selected language and the query without `lang`.
///
/// Every `lang` parameter must name the same supported language; otherwise the
/// selection is ignored and the URL is left unchanged.
fn explicit_locale(query: &str) -> Option<(Locale, String)> {
    let mut selected = None;
    let mut remaining = Vec::new();
    for segment in query.split('&').filter(|segment| !segment.is_empty()) {
        let (name, value) = segment.split_once('=').unwrap_or((segment, ""));
        if name != LANGUAGE_QUERY_PARAMETER {
            remaining.push(segment);
            continue;
        }
        let locale = Locale::from_code(value)?;
        if selected.is_some_and(|previous| previous != locale) {
            return None;
        }
        selected = Some(locale);
    }
    selected.map(|locale| (locale, remaining.join("&")))
}

/// HTML page routes are every route outside the API and static asset trees.
pub(crate) fn is_page_path(path: &str) -> bool {
    !path.starts_with("/api/v1") && !path.starts_with("/assets/")
}

/// Apply explicit language selection and language headers to HTML pages.
///
/// This runs inside the local security middleware, so a denied request never
/// reaches it and every response here still receives the security headers.
pub(crate) async fn negotiate_page_language(
    State(policy): State<SecurityPolicy>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();
    if !is_page_path(&path) {
        return next.run(request).await;
    }
    if matches!(*request.method(), Method::GET | Method::HEAD) {
        if let Some((locale, remaining)) = request.uri().query().and_then(explicit_locale) {
            // The canonical origin keeps the redirect same-origin even for a
            // path such as `//example.test` that reached the not-found page.
            let mut location = format!("{}{path}", policy.canonical_origin());
            if !remaining.is_empty() {
                location.push('?');
                location.push_str(&remaining);
            }
            let mut response = StatusCode::SEE_OTHER.into_response();
            let headers = response.headers_mut();
            if let Ok(location) = HeaderValue::from_str(&location) {
                headers.insert(LOCATION, location);
            } else {
                return next.run(request).await;
            }
            headers.insert(
                SET_COOKIE,
                HeaderValue::from_str(&format!(
                    "{LANGUAGE_COOKIE}={}; {LANGUAGE_COOKIE_ATTRIBUTES}",
                    locale.code()
                ))
                .expect("the language cookie is a valid header value"),
            );
            headers.insert(VARY, HeaderValue::from_static(PAGE_VARY));
            return response;
        }
    }
    let locale = resolve_locale(request.headers());
    request.extensions_mut().insert(locale);
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(VARY, HeaderValue::from_static(PAGE_VARY));
    let is_html = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"));
    if is_html {
        headers.insert(CONTENT_LANGUAGE, HeaderValue::from_static(locale.code()));
    }
    response
}

/// Declare a catalog section whose Japanese and English entries sit together.
///
/// A missing language is a macro parse error and a missing field is a struct
/// initialization error, so an untranslated entry cannot compile.
macro_rules! message_section {
    ($(#[$meta:meta])* $name:ident { $($field:ident: $ja:literal, $en:literal;)+ }) => {
        $(#[$meta])*
        pub(crate) struct $name {
            $(pub(crate) $field: &'static str,)+
        }

        impl $name {
            const JA: Self = Self { $($field: $ja,)+ };
            const EN: Self = Self { $($field: $en,)+ };

            #[cfg(test)]
            fn entries(&self) -> Vec<(&'static str, &'static str)> {
                vec![$((stringify!($field), self.$field)),+]
            }
        }
    };
}

message_section! {
    /// Page chrome and phrases shared by several pages.
    Common {
        skip_link: "本文へ移動", "Skip to main content";
        primary_nav: "メインナビゲーション", "Main navigation";
        nav_projects: "プロジェクト", "Projects";
        language_nav: "表示言語", "Display language";
        breadcrumb: "パンくずリスト", "Breadcrumb";
        state_complete: "完了", "Completed";
        state_pending: "レビュー待ち", "Awaiting review";
        state_incomplete: "未完了", "Incomplete";
        waiting_to_load: "読み込み待ち", "Waiting to load";
        download_verified: "確認済みの元ファイルをダウンロード", "Download the verified original file";
        download_image: "画像をダウンロード", "Download image";
        pinned_proposal: "固定したProposalの版", "Pinned Proposal version";
        pinned_decision: "固定したDecisionの版", "Pinned Decision version";
        original_decision_prefix: " · 元の判断: ", " · Original decision: ";
        note_tool: "使用ツール", "Tool used";
        note_model: "モデル名", "Model name";
        note_prompt: "プロンプト", "Prompt";
        note_intent: "制作意図", "Intent";
        new_session_heading: "新しいセッション", "New session";
        new_session_label: "新しいセッション名", "New session name";
        field_creator_name: "作成者名", "Creator name";
        field_subject_label: "対象名", "Subject label";
        event_label: "イベント", "event";
        clear_selection: "選択を解除", "Clear selection";
        create_proposal: "提案を作成", "Create proposal";
        zoom_fit: "全体を表示", "Fit to view";
        review_in_new_session: "この提案を新しいセッションでレビューする", "Review this proposal in a new session";
        decision_finality: "Deferも含め、記録後にこのセッションの判断を変更・再開する機能はありません。", "Every disposition, including Defer, completes this session. A recorded decision cannot be changed or reopened in this session.";
    }
}

message_section! {
    /// Project list and archive listing.
    Index {
        page_title: "プロジェクト", "Projects";
        eyebrow: "このコンピューター上だけで動作中", "Running only on this computer";
        title: "制作履歴を、手元で確かめる", "Check your creative history locally";
        intro: "画像、AIの提案、人の判断をローカルの不変履歴として確認できます。外部への公開や同期は行いません。", "Review images, AI proposals, and human decisions as local, immutable history. Nothing is published or synced externally.";
        projects_help: "起動時に登録された正確なローカルリポジトリのみを表示します。", "Only the exact local repositories registered at startup are shown.";
        no_projects_heading: "登録されたプロジェクトがありません", "No projects are registered";
        no_projects_before: "サーバー起動時に ", "Start the server with ";
        no_projects_after: " を指定してください。", ".";
        completed_sessions: "完了セッション", "Completed sessions";
        archives_heading: "アーカイブ", "Archives";
        archives_help_listing: "起動時に指定したアーカイブの保存先にあるものを一覧します（読み取りのみ）。書き出しと、空のプロジェクトへの復元は、各プロジェクトの「管理」から行います。", "Lists what is in the archive location set at startup (read-only). Export, and restore into an empty project, start from each project's Maintenance page.";
        archives_help_evidence: "状態は、アーカイブの目録の構造とチェックサムだけを確かめた結果です。中のデータの再確認や、復元の成功を保証するものではありません。", "Each state only checks the structure and checksum of the archive's list of contents. It does not re-check the stored data or guarantee a successful restore.";
        archives_error_heading: "アーカイブの一覧を読み込めません", "The archive list could not be loaded";
        no_archives_heading: "表示できるアーカイブがありません", "No archives to show";
        no_archives_before: "サーバー起動時に ", "Start the server with ";
        no_archives_after: " を指定し、そのディレクトリ直下に書き出したアーカイブを置いてください。", " and place exported archives directly inside that directory.";
        project_ready: "利用可能", "Available";
        project_empty_restore_target: "空の復元先", "Empty restore target";
        project_unavailable: "利用不可", "Unavailable";
        archive_staging_or_unknown: "作成途中または不明", "Being written or unknown";
    }
}

message_section! {
    /// Project dashboard, import forms, session list, and maintenance.
    Project {
        local_available: "ローカル・利用可能", "Local · available";
        subnav_label: "プロジェクト内のページ", "Project pages";
        tab_sessions: "セッション", "Sessions";
        tab_import: "取り込む", "Import";
        tab_maintenance: "管理", "Maintenance";
        tab_history: "履歴", "History";
        inbox_ready_notice: "取り込み待ちの候補があります。「取り込む」で確認してください。", "Candidates are waiting to be imported. Review them under Import.";
        create_public_notes: "公開用の制作ノートを作る", "Create public production notes";
        overview_label: "プロジェクト概要", "Project overview";
        inbox_eyebrow: "スクリプトが書き出した候補", "Candidates written by a script";
        inbox_heading: "取り込み待ち", "Waiting to import";
        inbox_session: "Inboxのセッション名", "Inbox session";
        inbox_creator_name: "Inboxの作成者名", "Inbox creator name";
        inbox_subject_label: "Inboxの対象名", "Inbox subject label";
        inbox_generation_tool: "Inboxの使用ツール", "Inbox generation tool";
        inbox_generation_model: "Inboxのモデル名", "Inbox generation model";
        inbox_generation_prompt: "Inboxのプロンプト", "Inbox generation prompt";
        inbox_generation_intent: "Inboxの制作意図", "Inbox generation intent";
        inbox_help: "候補を確認すると、その時点の3ファイルを確かめて一時保存し、その内容だけを提案として取り込みます。", "When you review a candidate, its three files are checked and set aside, and only those exact contents are imported as a proposal.";
        inbox_review_heading: "取り込み内容を確認", "Review the import";
        inbox_warning: "AI outputと生成メモは外部で用意されたものです。このアプリが作ったものではありません。あなたが判断するまで、判断は記録されません。", "The AI output and generation note were prepared outside this app; this app did not make them. No decision is recorded until you make one.";
        back_to_list: "一覧へ戻る", "Back to the list";
        upload_eyebrow: "提案の作成まで（判断は次の画面）", "Creates the proposal; you decide on the next page";
        upload_heading: "3つのファイルから始める", "Start from three files";
        session_label: "新しいセッション名", "Session";
        original_image: "Original画像", "Original image";
        current_image: "Current画像", "Current image";
        ai_output_file: "AI output（外部で作成）", "AI output (made outside this app)";
        upload_help: "3つのファイルをこのコンピューター内に取り込んで提案を記録し、人のレビューへ進みます。外部へは送信しません。", "Imports the three files on this computer, records the proposal, and continues to human review. Nothing is sent elsewhere.";
        upload_warning_heading: "AI outputはこのアプリが作ったものではありません", "This app did not make the AI output";
        upload_warning: "外部のツールやAIで作ったファイルを、そのまま記録します。判断の前に、3つの画像と、ファイル内容の一致確認を必ず見てください。", "The file you made with another tool or AI is recorded as it is. Before deciding, always check all three images and the file-content match check.";
        upload_busy: "ファイルを取り込み、提案を記録しています…", "Importing the files and recording the proposal…";
        upload_success: "提案を記録しました。レビュー画面へ移動します。", "Proposal recorded. Opening the review page.";
        session_hint: "小文字英数字とハイフン、1–64文字。新しい名前で作成します。", "Lowercase letters, digits, and hyphens; 1–64 characters. Use a new name.";
        creator_name_hint: "UTF-8で300 bytes以内。", "Up to 300 UTF-8 bytes.";
        subject_hint: "UTF-8で500 bytes以内。", "Up to 500 UTF-8 bytes.";
        preview_note: "ローカルプレビューです。ファイルを選ぶだけでは送信・保存されません。Original／Current／AI outputの取り違えがないか確認してください。", "Local preview only. Choosing a file does not send or save it. Check that Original, Current, and AI output are not mixed up.";
        raw_bytes_hint: "ファイルはそのまま保存します。64 MiB以内。", "Saved exactly as the file is; up to 64 MiB.";
        js_write_hint: "安全に保存するため、このページのJavaScriptが必要です。", "Saving safely needs this page's JavaScript.";
        upload_noscript: "JavaScriptが無効なため、この画面からは取り込めません。表示だけできます。", "JavaScript is disabled, so you cannot import from this page. You can still read it.";
        sessions_heading: "セッション", "Sessions";
        sessions_help: "レビュー待ちを優先し、履歴の更新が新しい順に最大200件を表示します。概要は未検証です。取得できない項目は推測せず、セッションを開いて詳細を検証・確認できます。記録日時は制作時刻を証明するものではありません。", "Shows up to 200 sessions: those awaiting review first, then by most recent history update. Summaries are unverified. Items that cannot be read are not guessed; open a session to verify its details. Recorded times do not prove when the work was made.";
        locator_heading: "名前でセッションを開く", "Open a session by name";
        locator_help: "一覧は最大200件です。古いセッションも、完全に一致する名前で開けます。", "The list shows at most 200 sessions. Open an older session by its exact name.";
        locator_label: "セッション名", "Session name";
        locator_submit: "セッションを開く", "Open session";
        locator_hint: "小文字英数字とハイフン、1–64文字。大文字小文字を区別します。", "Lowercase letters, digits, and hyphens; 1–64 characters. Names are case-sensitive.";
        locator_invalid: "小文字で始まり、小文字英数字またはハイフンだけを使った1–64文字の名前を入力してください。", "Enter a 1–64 character name that starts with a lowercase letter and uses only lowercase letters, digits, or hyphens.";
        locator_noscript: "古いセッションを名前で開くにはJavaScriptを有効にしてください。一覧は最大200件です。", "Enable JavaScript to open an older session by name. The list shows at most 200 sessions.";
        filter_label: "状態・判断で絞り込む", "Filter by state or decision";
        filter_all: "すべて", "All";
        no_sessions_heading: "セッションはまだありません", "No sessions yet";
        no_sessions_help: "「取り込む」で3つのファイルを取り込むと、最初の提案とレビューを始められます。", "Import three files under Import to start the first proposal and review.";
        column_session: "セッション／内容", "Session / content";
        column_state: "状態・判断", "State / decision";
        column_recorded_at: "記録日時", "Recorded at";
        column_source: "派生元", "Derived from";
        unavailable_value: "取得できません", "Unavailable";
        no_reflog_message: "メッセージなし", "No message";
        maintenance_heading: "メンテナンス", "Maintenance";
        fsck_heading: "リポジトリ整合性の確認", "Check repository integrity";
        fsck_help: "現在の履歴と保存データの整合性を、上限を設けて読み取り専用で確認します。", "Checks the consistency of the current history and stored data, read-only and within fixed limits.";
        last_fsck_prefix: "このアプリを起動してからの直近の結果: ", "Latest result since this app started: ";
        fsck_clean: "問題なし", "clean";
        fsck_issues_found: "問題あり", "issues found";
        fsck_verified_objects: "確認したデータ: ", "Stored items checked: ";
        fsck_issues: " · 問題: ", " · Issues: ";
        fsck_busy: "整合性の確認を開始しています…", "Starting the integrity check…";
        confirm_key_before: "確認のため project key ", "To confirm, type the project key ";
        confirm_key_after: " を入力", "";
        fsck_submit: "整合性を確認する（読み取りのみ）", "Check integrity (read-only)";
        fsck_hint: "処理はバックグラウンドで実行します。履歴や保存データを変更しません。", "It runs in the background and does not change history or stored data.";
        fsck_noscript: "JavaScriptが無効なため、メンテナンス操作は開始できません。", "JavaScript is disabled, so maintenance operations cannot be started.";
        export_heading: "アーカイブを書き出す", "Export an archive";
        export_help: "現在のプロジェクトを、設定済みの保存先へチェックサム付きで保存します。", "Saves the current project, with checksums, to the configured archive location.";
        export_warning_heading: "既存のアーカイブは上書きしません", "Existing archives are never overwritten";
        export_warning: "新しいアーカイブ名を指定してください。保存先は起動時の設定で固定され、この画面から変更できません。", "Enter a new archive name. The destination is fixed by the startup configuration and cannot be changed from this page.";
        export_busy: "アーカイブの書き出しを開始しています…", "Starting the archive export…";
        archive_name: "アーカイブ名", "Archive name";
        archive_name_hint: "小文字英数字とハイフン、1–64文字。新しい名前で作成します。", "Lowercase letters, digits, and hyphens; 1–64 characters. Use a new name.";
        confirm_value_hint: "操作対象を確認するため、表示された値をそのまま入力してください。", "Type the value shown exactly, to confirm the target.";
        export_submit: "アーカイブを作成", "Create archive";
        export_hint: "処理はバックグラウンドで実行し、成功後にアーカイブ一覧へ移動します。", "It runs in the background and opens the archive list when it succeeds.";
        export_noscript: "JavaScriptが無効なため、アーカイブの書き出しは開始できません。", "JavaScript is disabled, so an archive export cannot be started.";
        restore_heading: "アーカイブを復元する", "Restore an archive";
        restore_help: "保存したアーカイブを、このプロジェクトへ復元します。", "Restores a saved archive into this project.";
        restore_warning_heading: "履歴がないプロジェクトへ復元します", "Restores only into a project without history";
        restore_warning_before: "アーカイブの状態は", "Check the archive state in the ";
        restore_warning_link: "アーカイブ一覧", "Archives list";
        restore_warning_after: "で確認してください。", ".";
        restore_busy: "アーカイブの復元を開始しています…", "Starting the archive restore…";
        restore_name_hint: "アーカイブ一覧の名前を小文字のまま入力してください。", "Type the name from the Archives list, in lowercase.";
        restore_confirm_before: "確認のため復元先の project key ", "To confirm, type the restore target's project key ";
        restore_confirm_after: " を入力", "";
        restore_empty_check: "このプロジェクトに既存の履歴がないことを確認しました。", "I have confirmed that this project has no existing history.";
        restore_submit: "アーカイブを復元", "Restore archive";
        restore_hint: "失敗時や結果不明の場合は、ファイルの一部がコピー済みの可能性があります。自動では再試行しません。再試行する場合は同じアーカイブを使ってください。", "If the restore fails or its outcome is unknown, some files may already have been copied. It is not retried automatically. To retry, use the same archive.";
        restore_success_link: "復元した履歴を確認", "View the restored history";
        restore_unavailable_heading: "このプロジェクトは復元先にできません", "This project cannot be a restore target";
        restore_unavailable: "既存の履歴があります。復元用には別の空のプロジェクトを初期化・登録してから開いてください。", "It already has history. For a restore, initialize and register a separate empty project, then open that project.";
        restore_noscript: "JavaScriptが無効なため、アーカイブの復元は開始できません。", "JavaScript is disabled, so an archive restore cannot be started.";
        reflog_heading: "最近の変更（reflog）", "Recent changes (reflog)";
        history_intro: "確認や調査のための技術情報です。記録の名前付きの版（Ref）と、最近の変更（reflog）を表示します。制作の流れは各セッションのタイムラインで確認できます。", "Technical information for checking and investigation: the named record versions (Refs) and recent changes (reflog). Each session's Timeline shows the creative steps.";
        show_heads: "版の識別子", "Version identifiers";
    }
}

message_section! {
    /// Optional caller-declared generation note fields.
    Note {
        summary: "提案の生成メモ（任意）", "Proposal generation note (optional)";
        help: "このAI outputについてあなたが書き残す非公開のメモです。実行の記録や、モデルが動いた証明ではありません。通常のアーカイブには含まれます。入力内容と上のAI outputを確認してから提案を作成してください。", "A private note you write about this AI output. It is not an execution log or proof that a model ran. It is included in normal archives. Check what you entered and the AI output above before creating the proposal.";
        short_label_tool: "使用ツール", "Tool used";
        short_label_model: "モデル", "Model";
    }
}

message_section! {
    /// Creator session review, decision, evidence, pins, and diagnostics.
    Session {
        eyebrow: "Creatorセッション", "Creator session";
        identity_subject: "対象", "Subject";
        identity_creator: "作成者", "Creator";
        identity_recorded_at: "判断を記録した時刻", "Decision recorded at";
        identity_hint: "対象と作成者は記録時に入力された表示名で、本人性の証明ではありません。記録した時刻は、判断した時刻の証明ではありません。", "The subject and creator are display names entered when recording and do not prove identity. The recording time does not prove when the decision was made.";
        not_recorded: "記録なし", "Not recorded";
        metric_disposition: "判断", "Disposition";
        metric_ai_selected: "AI outputの選択", "AI output selected";
        metric_verified_objects: "確認したデータ数", "Stored items checked";
        decision_eyebrow: "人の判断", "Human Decision";
        gate_eyebrow: "人の判断", "Human gate";
        original_blob: "Originalの識別子", "Original identifier";
        current_blob: "Currentの識別子", "Current identifier";
        evidence_heading: "ファイル内容の一致確認", "File-content match check";
        evidence_status: "確認の結果", "Check result";
        evidence_comparability: "比較できた範囲（comparability）", "Comparability";
        evidence_adapter: "確認に使った方式（adapter）", "Method used (adapter)";
        evidence_replay: "再確認の準備（replay）", "Ready to re-check (replay)";
        evidence_recorded_warning: "確認方式が記録した注意（英語）", "Caveat recorded by the method";
        diagnostics_label: "Creatorセッションの診断", "Creator session diagnostics";
        automatic_resume: "自動再開", "Automatic resume";
        automatic_cleanup: "自動クリーンアップ", "Automatic cleanup";
        timeline_heading: "タイムライン", "Timeline";
        try_next: "この記録から次の案を試す", "Try a next candidate from this record";
        rereview_deferred: "保留した提案を改めて判断する", "Re-review the deferred proposal in a new session";
        result_label: "レビュー結果", "Review result";
        recorded_decision: "記録した判断", "Recorded decision";
        recorded_rationale: "記録された理由", "Recorded rationale";
        no_rationale: "理由は記録されていません。", "No rationale was recorded.";
        recorded_hint: "保存された履歴から確認した記録です。理由は記録されたテキストで、その内容が正しいことは確認していません。", "This record was checked against the stored history. The rationale is recorded text; its content has not been checked for correctness.";
        private_report_heading: "記録を保存", "Save this record";
        private_report_noscript: "記録を保存するにはJavaScriptを有効にしてください。", "Enable JavaScript to save this record.";
        private_report_help: "理由、生成メモ、ピン、内部の識別子を含む非公開の記録を、ご自身で保管するために保存できます。画像やプロジェクト全体は含まれないため、バックアップや公開・共有用のファイルではありません。", "Save a private record for your own keeping. It can include rationale, generation notes, pins, and internal identifiers. It contains neither images nor the whole project, so it is not a backup or a file for publishing or sharing.";
        private_report_download: "非公開の記録を保存（JSON）", "Save private record (JSON)";
        review_required: "人のレビューが必要です", "Human review required";
        source_before: "AI outputの出どころ: ", "Where the AI output came from: ";
        source_after: "。このアプリや特定のAIモデルが作ったことを示すものではありません。", ". It was prepared outside this app; this does not show that this app or any particular AI model made it.";
        review_instruction: "3つの画像とファイル内容の一致確認を見てから、採用・不採用・保留のいずれかを選んでください。", "Check the three images and the file-content match check, then choose adopt, reject, or defer.";
        derived_source_heading: "再利用した参照画像の派生元", "Source of the reused reference images";
        derived_source_help: "元のOriginal／Currentとまったく同じファイルを参照として再利用しました。新しく撮影・観測したものではありません。元のAI outputをCurrentに置き換えてはいません。表示名が同じでも、同じ人・同じ対象物であることの証明にはなりません。", "The exact Original and Current files of the source were reused as references. They are not a new capture or observation. The source's AI output did not replace Current. Matching display names do not prove the same person or the same physical subject.";
        derived_source_hint: "派生元のリンク先は現在の記録を表示します。このセッションは、技術的な詳細にある固定した版との対応を保っています。", "The source link shows the current record. This session keeps its link to the pinned versions under Technical details.";
        reuse_source_heading: "記録済みの提案を引き継ぎました", "A recorded proposal was carried over";
        reuse_source_link_prefix: "元のセッション ", "Original session ";
        reuse_source_after: " の3つの画像を確認用に使っています。", ": its three images are used for this review.";
        reuse_source_help: "新しい生成や元の判断の変更ではありません。元の理由とメモは参照用で、新しい判断にはコピーされません。", "This is not a new generation and does not change the original decision. The original rationale and notes are for reference only and are not copied into the new decision.";
        original_proposal: "元のProposalの版", "Original Proposal version";
        original_decision: "元のDecisionの版", "Original Decision version";
        reference_heading: "元の記録（参照のみ）", "Original record (reference only)";
        reference_deferred: "元の保留した提案の情報です。新しいセッションのメモ、ピン、理由にはコピーされません。", "Information from the original deferred proposal. It is not copied into this session's notes, pins, or rationale.";
        reference_interrupted: "元の中断した提案の情報です。新しいセッションのメモ、ピン、理由にはコピーされません。", "Information from the original interrupted proposal. It is not copied into this session's notes, pins, or rationale.";
        reference_unavailable: "元の参照情報を取得できませんでした。", "The original reference information could not be loaded.";
        reference_note_heading: "元の生成メモ", "Original generation note";
        reference_defer_heading: "元のDefer理由", "Original Defer rationale";
        reference_pins_heading: "元の画像上の判断メモ", "Original image pins";
        note_heading: "提案の生成メモ", "Proposal generation note";
        note_hint: "利用者が書き残した非公開のメモです。モデルが動いたことや、作者が誰かの証明ではありません。通常のアーカイブには含まれます。", "A private note written by the user. It does not prove that a model ran or who the author is. It is included in normal archives.";
        no_note: "生成メモなし", "No generation note";
        images_heading: "画像と出力", "Images and output";
        images_help: "画像は、確認済みのこのセッションの記録から読み込みます。", "The images are loaded from this session's verified record.";
        compare_open: "画像を拡大して比較", "Compare images at full size";
        compare_title: "画像を見比べる", "Compare images";
        compare_close: "閉じる", "Close";
        compare_help: "目視確認用です。位置合わせ・差分解析は行いません。100%は画像の1 pixelを1 CSS pixelで表示します。拡大時は各画像をスクロールできます。Escで閉じます。", "For visual checking only. No registration or difference analysis is performed. 100% shows one image pixel per CSS pixel. When zoomed in, each image can be scrolled. Press Esc to close.";
        zoom_label: "表示倍率", "Zoom";
        mode_legend: "表示方法", "Layout";
        mode_side_by_side: "並べて表示", "Side by side";
        mode_overlay: "重ねて表示", "Overlay";
        overlay_unavailable: "重ねて表示するには、A と B のデコード後の幅と高さが一致している必要があります。並べて表示で確認できます。", "Overlay needs A and B to have the same decoded width and height. You can still compare them side by side.";
        opacity_label: "画像 B の不透明度 ", "Image B opacity ";
        opacity_hint: "画像 B 本来の透明部分はそのまま保持します。", "Image B keeps its own transparent areas.";
        source_a: "比較画像 A", "Comparison image A";
        source_b: "比較画像 B", "Comparison image B";
        viewport_a: "比較画像 Aの表示領域", "Comparison image A viewport";
        viewport_b: "比較画像 Bの表示領域", "Comparison image B viewport";
        overlay_viewport: "重ねた比較画像の表示領域", "Overlaid comparison viewport";
        overlay_canvas: "比較画像 A を下、画像 B を上に重ねた表示", "Comparison image A below, with image B overlaid on top";
        pins_heading: "画像上の判断メモ", "Image pins";
        pins_help: "レビューする人が画像に付ける非公開のメモです。採用・不採用・保留は提案全体への判断で、ピンは部分的な採用や画像解析を表しません。通常のアーカイブには含まれます。", "Private notes that the reviewer pins to an image. Adopt, Reject, and Defer apply to the whole proposal; pins do not mean partial adoption or image analysis. They are included in normal archives.";
        pins_unavailable: "ピンの形式が不明か、画像との対応が正しくないため、表示できません。判断の記録そのものの確認とは別の問題です。", "Pins cannot be shown because their format is unknown or their image link is invalid. This is separate from checking the decision record itself.";
        pin_target: "ピンの対象画像", "Pin target image";
        pin_zoom: "ピン画像の表示倍率", "Pin image zoom";
        pin_viewport: "ピン画像の表示領域", "Pin image viewport";
        pin_image_alt: "判断メモの対象画像", "Image for the pins";
        pin_add: "中央にピンを追加", "Add a pin at the center";
        pin_help: "画像をクリック／タップして追加できます。ピンはドラッグまたは矢印キーで移動（Shiftで細かく移動）、Deleteで削除できます。一覧の座標でも移動できます。左上が(0, 0)、右下が(1000000, 1000000)です。最大10件、各メモは200バイト（UTF-8）以内。", "Click or tap the image to add a pin. Move a pin by dragging it or with the arrow keys (hold Shift for fine steps), and remove it with Delete. You can also move it with the coordinates in the list. The top left is (0, 0) and the bottom right is (1000000, 1000000). Up to 10 pins, each note up to 200 UTF-8 bytes.";
        no_pins: "記録されたピンなし", "No pins recorded";
        evidence_limit: "ファイルの内容を比べた結果で、見た目の比較ではありません。同じ内容でも対象物が変わっていないことの証明にはならず、内容が違っても見た目や対象物が変わったことの証明にはなりません。", "This compares file contents; it is not a visual comparison. Identical files do not prove that the physical subject is unchanged, and different files do not prove a visual or physical change.";
        no_comparison_heading: "一致確認の情報なし", "No match-check information";
        no_comparison: "この記録からは、ファイル内容の一致確認を再現できません。", "The file-content match check cannot be reconstructed from this record.";
        incomplete_heading: "セッションは未完了です", "This session is incomplete";
        cannot_resume_reuse: "このセッションの判断は再開できません。記録済みの3画像を引き継いで、新しいセッションでレビューできます。", "This session's decision cannot be resumed. You can carry over its three recorded images and review them in a new session.";
        cannot_resume_no_reuse: "このセッションの判断は再開できません。記録済みの画像を引き継いだレビューは現在利用できません。", "This session's decision cannot be resumed. A review that carries over the recorded images is not available now.";
        no_automatic_change: "記録を自動で書き換えたり削除したりはしません。問題が疑われる場合は、プロジェクトの「管理」で整合性を確認してください。", "Records are never rewritten or deleted automatically. If you suspect a problem, check integrity on the project's Maintenance page.";
        not_supported: "サポートされません", "Not supported";
        decision_heading: "判断を記録", "Record a decision";
        decision_busy: "判断を確認して記録しています…", "Checking and recording the decision…";
        decision_success: "判断を記録しました。ページを再読み込みします。", "Decision recorded. Reloading the page.";
        rationale_label: "理由（任意）", "Rationale (optional)";
        rationale_placeholder: "この判断の理由や、後から確認したい点", "Why you made this decision, or what to check later";
        rationale_hint: "UTF-8で5000 bytes以内。記録後、このセッションで読み返せます。", "Up to 5000 UTF-8 bytes. You can read it again in this session after it is recorded.";
        adopt_heading: "採用", "Adopt";
        adopt_help: "AI outputを変更せず採用します。", "Adopt the AI output without changes.";
        reject_heading: "不採用", "Reject";
        reject_help: "AI outputを採用しない判断を記録します。", "Record a decision not to adopt the AI output.";
        defer_heading: "保留", "Defer";
        defer_help: "AI outputの採用を保留する判断を記録します。", "Record a decision to defer adopting the AI output.";
        decision_js_hint: "判断を安全に記録するため、このページのJavaScriptが必要です。", "Recording a decision safely needs this page's JavaScript.";
        decision_noscript: "JavaScriptが無効なため、この画面からは判断を記録できません。3つの画像と確認結果は表示できます。", "JavaScript is disabled, so a decision cannot be recorded from this page. The three images and the check results remain readable.";
        derived_heading: "このセッションから派生したセッション", "Sessions derived from this session";
        derived_hint: "未確認の一覧です。派生先を開くと、保存された履歴から詳細を確認します。", "An unverified list. Opening a derived session checks its details against the stored history.";
        pending_description: "このアプリの起動中、人のレビューを待っています。", "Waiting for human review while this app is running.";
        interrupted_description: "判断前に中断されたセッションです。記録済みの画像を新しいセッションで確認できます。", "This session was interrupted before a decision. Its recorded images can be reviewed in a new session.";
        unverifiable_description: "現在の記録を確認できません。プロジェクトの「管理」で整合性を確認してください。", "The current record cannot be verified. Check integrity on the project's Maintenance page.";
        interrupted_diagnostic: "判断前に中断された記録です。記録済みの3画像を確認し、新しいセッションでレビューできます。", "This record was interrupted before a decision. You can check its three recorded images and review them in a new session.";
        complete_description: "保存された履歴から確認したレポートです。", "A report checked against the stored history.";
        outcome_adopt: "AI outputを変更せず採用しました。", "The AI output was adopted without changes.";
        outcome_reject: "AI outputを採用しない判断を記録しました。", "A decision not to adopt the AI output was recorded.";
        outcome_defer: "AI outputの採用を保留する判断を記録しました。", "A decision to defer adopting the AI output was recorded.";
        outcome_unknown: "確認済みレポートの判断を見てください。", "Check the decision in the verified report.";
        yes: "はい", "Yes";
        no: "いいえ", "No";
        original_alt: "取り込まれたoriginal画像", "Imported original image";
        current_alt: "取り込まれたcurrent画像", "Imported current image";
        ai_output_alt: "外部で作成したAI output", "AI output made outside this app";
    }
}

message_section! {
    /// Derived session with reused reference images.
    Derive {
        page_title: "次の案を試す", "Try a next candidate";
        heading: "この記録から次の案を試す", "Try a next candidate from this record";
        intro: "完了した記録の参照画像を引き継ぎ、新しい候補を1点取り込みます。", "Carry over the reference images of a completed record and import one new candidate.";
        source_heading: "派生元と再利用する参照画像", "Source and reused reference images";
        source_help: "元のOriginal／Currentとまったく同じファイルを再利用します。採用済みでも、元のAI outputをCurrentに置き換えません。新しく撮影・観測した現況ではなく、過去の参照画像です。", "The exact Original and Current files of the source are reused. Even after Adopt, the source's AI output does not replace Current. These are past reference images, not a new capture or observation of the current state.";
        original_label: "Original（記録済み参照）", "Original (recorded reference)";
        original_alt: "派生元のOriginal参照画像", "Original reference image from the source";
        current_label: "Current（記録済み参照）", "Current (recorded reference)";
        current_alt: "派生元のCurrent参照画像", "Current reference image from the source";
        new_session_help: "表示名を確認・編集してください。このセッションだけの表示名として記録します。同じ名前でも、同じ人・同じ対象物とは見なしません。元の生成メモや画像上の判断メモ（ピン）は引き継ぎません。", "Check and edit the display names. They are recorded for this session only. The same name is not taken to mean the same person or the same physical subject. The source's generation note and image pins are not carried over.";
        busy: "派生元を確認し直し、新しい提案を作成しています…", "Re-checking the source and creating a new proposal…";
        success: "新しい提案を作成しました。", "Created a new proposal.";
        session_hint: "未使用の小文字英数字・ハイフン、1〜64文字。", "An unused name of lowercase letters, digits, and hyphens; 1–64 characters.";
        preview_note: "新しい候補は必ず自分で選択してください。選ぶだけでは送信・保存されません。", "Always choose the new candidate yourself. Choosing a file does not send or save it.";
        file_label: "新しいAI output（外部で作成）", "New AI output (made outside this app)";
        preview_alt: "新しい候補のローカルプレビュー", "Local preview of the new candidate";
        clear_label: "新しいAI outputの選択を解除", "Clear the new AI output selection";
        submit: "参照画像を引き継いで提案を作成", "Create a proposal with the reference images";
        submit_hint: "派生元が確認後に変わった場合は作成を拒否します。元の判断は編集・再開しません。", "If the source changes after this check, creation is refused. The original decision is not edited or reopened.";
        noscript: "JavaScriptを有効にすると、安全な取り込みフォームを利用できます。", "Enable JavaScript to use the safe import form.";
    }
}

message_section! {
    /// Re-review of an interrupted or deferred proposal in a new session.
    Reuse {
        page_title: "記録済み候補を再レビュー", "Re-review a recorded candidate";
        breadcrumb: "再レビュー", "Re-review";
        heading: "記録済みの提案を新しいセッションでレビュー", "Review a recorded proposal in a new session";
        intro: "元の判断は再開・変更しません。記録済みの3画像を確認し、新しいセッションで判断します。", "The original decision is not reopened or changed. Check the three recorded images and decide in a new session.";
        record_heading: "引き継ぐ記録", "Record to carry over";
        from_interrupted: "中断した提案から画像を引き継ぎます。", "The images are carried over from an interrupted proposal.";
        from_deferred: "保留した提案から画像を引き継ぎます。", "The images are carried over from a deferred proposal.";
        not_regenerated: " AI outputを再生成したものではありません。", " The AI output is not regenerated.";
        deferred_rationale_prefix: "元のDefer理由（参照のみ）: ", "Original Defer rationale (reference only): ";
        note_heading: "元の生成メモ（参照のみ）", "Original generation note (reference only)";
        pins_heading: "元の画像上の判断メモ（参照のみ）", "Original image pins (reference only)";
        submit_hint: "確認した後に元の記録が変わった場合は、作成しません。", "If the original record changes after this check, creation is refused.";
    }
}

message_section! {
    /// Author-supplied public presentation sidecar form.
    Presentation {
        page_title: "公開用の制作ノート", "Public production notes";
        intro: "自分で入力した公開用の文章から、既存のコマンドで使う説明文ファイルを作ります。文章はすべてあなたが入力したものです。", "Create a description file for the existing command from public text that you enter yourself. All of the text is yours.";
        input_heading: "公開用の文章を入力", "Enter the public text";
        derived_unsupported: "参照画像を再利用した派生セッションは、現在の公開形式に未対応です。通常の3画像取り込みで作成したセッションを選択してください。", "Derived sessions that reuse reference images are not supported by the current publication format. Choose a session created by a normal three-image import.";
        private_not_copied: "保存済みの非公開の理由・プロンプト・生成メモ・画像上の判断メモは、自動では転記しません。入力内容は下書きとして保存されません。", "Stored private rationales, prompts, generation notes, and image pins are never copied in automatically. What you enter is not saved as a draft.";
        no_sessions: "このプロジェクトには完了したセッションがありません。先に通常のHuman Decisionを完了してください。", "This project has no completed sessions. Complete a normal Human Decision first.";
        session_label: "完了したセッション", "Completed session";
        session_placeholder: "選択してください", "Choose a session";
        title_label: "作品タイトル（任意）", "Work title (optional)";
        summary_label: "概要（任意）", "Summary (optional)";
        creator_label: "公開用Creator表示名（任意）", "Public creator display name (optional)";
        agent_label: "公開用Proposal agent表示名（任意）", "Public proposal agent display name (optional)";
        session_title_label: "セッションのタイトル（任意）", "Session title (optional)";
        original_caption_label: "Originalのcaption（任意）", "Original caption (optional)";
        current_caption_label: "Currentのcaption（任意）", "Current caption (optional)";
        proposal_caption_label: "Proposalのcaption（任意）", "Proposal caption (optional)";
        decision_note_label: "公開用の判断メモ（任意）", "Public decision note (optional)";
        check_submit: "入力した文章を確認", "Check the entered text";
        noscript: "説明文の確認とダウンロードにはJavaScriptが必要です。", "Checking and downloading the description file requires JavaScript.";
        preview_heading: "公開用文章の確認", "Review the public text";
        preview_help: "自分で入力した文章だけの確認です。最終的な公開用のファイル一式（bundle）を確認したプレビューではありません。", "This shows only the text you entered. It is not a verified preview of the final publication bundle.";
        download: "説明文ファイルを書き出す", "Export the description file";
        next_heading: "次の手順：bundleを生成・検証する", "Next: generate and verify the bundle";
        next_help: "ダウンロードはpresentation.tomlの作成だけです。画像・thumbnailを含めず、Coreの記録を書き換えません。外部への公開・共有も行いません。", "The download only creates presentation.toml. It contains no images or thumbnails, does not rewrite Core records, and does not publish or share anything externally.";
        step_stop_writers: "localhostアプリを含むsourceへのwriterを停止し、checkpoint済みのsourceを用意します。", "Stop every writer to the source, including this localhost app, and prepare a checkpointed source.";
        step_export: "既存CLIのsynapse-present exportで、ダウンロードしたファイルを--presentationに指定します。", "Run the existing CLI command synapse-present export with the downloaded file as --presentation.";
        step_preview: "synapse-present previewで生成したbundleを検証します。read_only_source_busyの場合はwriter停止とcheckpointを確認してください。", "Verify the generated bundle with synapse-present preview. If it reports read_only_source_busy, check that writers are stopped and the source is checkpointed.";
        step_share: "外部共有は生成物を確認した後の別操作です。", "Sharing externally is a separate step after you review the output.";
        example_intro: "実行例（SOURCE・BUNDLE・SESSIONは自分のローカルパスに置き換えます）：", "Example (replace SOURCE, BUNDLE, and SESSION with your own local values):";
    }
}

message_section! {
    /// HTML error page chrome.
    ErrorPage {
        breadcrumb: "エラー", "Error";
        server_detail: "サーバーからの詳細（英語）: ", "Server detail: ";
        request_label: "リクエストID", "request";
    }
}

message_section! {
    /// Readable Timeline stages, time bases, and dashboard time bases.
    TimelineText {
        stage_original: "元の状態を記録", "Original state recorded";
        stage_current: "現在の状態を記録", "Current state recorded";
        stage_import: "3つの画像を取り込み", "Three images imported";
        stage_proposal: "AI提案を記録", "AI proposal recorded";
        stage_decision: "人の判断を記録", "Human decision recorded";
        stage_other: "その他の記録", "Other record";
        basis_capture_instant: "撮影時刻（記録上の値）", "Capture time, as recorded";
        basis_capture_interval: "撮影期間（記録上の値）", "Capture interval, as recorded";
        basis_observation_fallback: "記録した時刻（撮影時刻は不明）", "Recording time; the capture time is unknown";
        basis_valid_instant: "実行時刻（記録上の値）", "Activity time, as recorded";
        basis_valid_interval: "実行期間（記録上の値）", "Activity interval, as recorded";
        basis_activity_fallback: "記録した時刻（実行時刻は不明）", "Recording time; the activity time is unknown";
        basis_decision: "記録した時刻（判断した時刻の証明ではない）", "Recording time, not proof of when the decision was made";
        summary_recorded: "判断を記録した時刻", "Decision recording time";
        summary_authored: "記録の作成時刻（未確認の代替）", "Record creation time, unverified fallback";
        summary_pending: "記録順の時刻", "Recording order time";
        technical_details: "技術的な詳細", "Technical details";
    }
}

message_section! {
    /// Display labels for known stored codes.  English keeps the stored code so
    /// the English interface is unchanged; unknown codes are shown as stored.
    Values {
        adopt: "採用", "adopt";
        reject: "不採用", "reject";
        defer: "保留", "defer";
        identical: "同一", "identical";
        different: "異なる", "different";
        not_compared: "比較なし", "not_compared";
        succeeded: "成功", "succeeded";
        not_run: "未実行", "not_run";
        partial: "部分的", "partial";
        incomparable: "比較不能", "incomparable";
        observation: "観測", "observation";
        activity: "活動", "activity";
        decision: "判断", "decision";
        caller_supplied: "外部で用意したもの", "caller_supplied";
    }
}

/// The complete interface catalog for one [`Locale`].
pub(crate) struct Messages {
    pub(crate) locale: Locale,
    pub(crate) common: Common,
    pub(crate) index: Index,
    pub(crate) project: Project,
    pub(crate) note: Note,
    pub(crate) session: Session,
    pub(crate) derive: Derive,
    pub(crate) reuse: Reuse,
    pub(crate) presentation: Presentation,
    pub(crate) error: ErrorPage,
    pub(crate) values: Values,
    pub(crate) timeline: TimelineText,
}

static JA: Messages = Messages {
    locale: Locale::Ja,
    common: Common::JA,
    index: Index::JA,
    project: Project::JA,
    note: Note::JA,
    session: Session::JA,
    derive: Derive::JA,
    reuse: Reuse::JA,
    presentation: Presentation::JA,
    error: ErrorPage::JA,
    values: Values::JA,
    timeline: TimelineText::JA,
};

static EN: Messages = Messages {
    locale: Locale::En,
    common: Common::EN,
    index: Index::EN,
    project: Project::EN,
    note: Note::EN,
    session: Session::EN,
    derive: Derive::EN,
    reuse: Reuse::EN,
    presentation: Presentation::EN,
    error: ErrorPage::EN,
    values: Values::EN,
    timeline: TimelineText::EN,
};

/// Parameterized and keyed messages. Every method takes both languages, so
/// a missing translation is a compile error here too.
impl Messages {
    const fn pick(&self, ja: &'static str, en: &'static str) -> &'static str {
        match self.locale {
            Locale::Ja => ja,
            Locale::En => en,
        }
    }

    fn count(&self, count: usize, one: &str, many: &str) -> String {
        match self.locale {
            Locale::Ja => format!("{count} 件"),
            Locale::En if count == 1 => format!("{count} {one}"),
            Locale::En => format!("{count} {many}"),
        }
    }

    pub(crate) fn count_projects(&self, count: impl Borrow<usize>) -> String {
        self.count(*count.borrow(), "project", "projects")
    }

    pub(crate) fn count_archives(&self, count: impl Borrow<usize>) -> String {
        self.count(*count.borrow(), "archive", "archives")
    }

    pub(crate) fn count_sessions(&self, count: impl Borrow<usize>) -> String {
        self.count(*count.borrow(), "session", "sessions")
    }

    pub(crate) fn count_candidates(&self, count: impl Borrow<usize>) -> String {
        self.count(*count.borrow(), "candidate", "candidates")
    }

    pub(crate) fn count_events(&self, count: impl Borrow<usize>) -> String {
        self.count(*count.borrow(), "event", "events")
    }

    pub(crate) fn local_preview_alt(&self, label: impl AsRef<str>) -> String {
        let label = label.as_ref();
        match self.locale {
            Locale::Ja => format!("{label}のローカルプレビュー"),
            Locale::En => format!("Local preview of {label}"),
        }
    }

    pub(crate) fn clear_selection_label(&self, label: impl AsRef<str>) -> String {
        let label = label.as_ref();
        match self.locale {
            Locale::Ja => format!("{label}の選択を解除"),
            Locale::En => format!("Clear the {label} selection"),
        }
    }

    pub(crate) fn note_limit(&self, bytes: impl Borrow<usize>) -> String {
        let bytes = *bytes.borrow();
        match self.locale {
            Locale::Ja => format!("UTF-8 {bytes} bytes以内"),
            Locale::En => format!("Up to {bytes} UTF-8 bytes"),
        }
    }

    pub(crate) fn single_line_limit(&self, bytes: impl Borrow<usize>) -> String {
        let bytes = *bytes.borrow();
        match self.locale {
            Locale::Ja => format!("最大{bytes} UTF-8 bytes。1行。"),
            Locale::En => format!("Up to {bytes} UTF-8 bytes. One line."),
        }
    }

    pub(crate) fn multi_line_limit(&self, bytes: impl Borrow<usize>) -> String {
        let bytes = *bytes.borrow();
        match self.locale {
            Locale::Ja => format!("最大{bytes} UTF-8 bytes。改行可。"),
            Locale::En => format!("Up to {bytes} UTF-8 bytes. Line breaks allowed."),
        }
    }

    pub(crate) fn recorded_role_alt(&self, role: impl AsRef<str>) -> String {
        let role = role.as_ref();
        match self.locale {
            Locale::Ja => format!("記録済み{role}"),
            Locale::En => format!("Recorded {role}"),
        }
    }

    pub(crate) fn yes_no(&self, value: bool) -> &'static str {
        if value {
            self.session.yes
        } else {
            self.session.no
        }
    }

    /// A display label for a known stored code, or the code itself.
    ///
    /// Only the interface label is localized; the stored value, API fields, and
    /// data attributes keep the code.
    pub(crate) fn value_label(&self, code: impl AsRef<str>) -> String {
        let code = code.as_ref();
        let values = &self.values;
        match code {
            "adopt" => values.adopt,
            "reject" => values.reject,
            "defer" => values.defer,
            "identical" => values.identical,
            "different" => values.different,
            "not_compared" => values.not_compared,
            "succeeded" => values.succeeded,
            "not_run" => values.not_run,
            "partial" => values.partial,
            "incomparable" => values.incomparable,
            "observation" => values.observation,
            "activity" => values.activity,
            "decision" => values.decision,
            "caller_supplied" => values.caller_supplied,
            _ => code,
        }
        .to_owned()
    }

    /// A readable Timeline stage; an unknown code is shown as stored.
    pub(crate) fn timeline_stage_label(&self, code: &str) -> String {
        let text = &self.timeline;
        match code {
            "original_observation" => text.stage_original,
            "current_observation" => text.stage_current,
            "image_import" => text.stage_import,
            "ai_proposal" => text.stage_proposal,
            "human_decision" => text.stage_decision,
            "other" => text.stage_other,
            _ => code,
        }
        .to_owned()
    }

    /// A readable Timeline time basis; an unknown code is shown as stored.
    pub(crate) fn timeline_basis_label(&self, code: &str) -> String {
        let text = &self.timeline;
        match code {
            "observation_capture_instant" => text.basis_capture_instant,
            "observation_capture_interval" => text.basis_capture_interval,
            "observation_recorded_at_fallback" => text.basis_observation_fallback,
            "activity_valid_instant" => text.basis_valid_instant,
            "activity_valid_interval" => text.basis_valid_interval,
            "activity_recorded_at_fallback" => text.basis_activity_fallback,
            "decision_recorded_at" => text.basis_decision,
            _ => code,
        }
        .to_owned()
    }

    /// A readable dashboard time basis; an unknown value is shown as stored.
    pub(crate) fn summary_basis_label(&self, code: &str) -> String {
        let text = &self.timeline;
        match code {
            "recorded_at" => text.summary_recorded,
            "authored_at (unverified fallback)" => text.summary_authored,
            "recorded ordering time" => text.summary_pending,
            _ => code,
        }
        .to_owned()
    }

    /// The display name of a stored image role.  Role names are glossary terms
    /// kept in both languages; this only normalizes the stored spelling.
    pub(crate) fn role_label(&self, role: impl AsRef<str>) -> String {
        match role.as_ref() {
            "original" => "Original",
            "current" => "Current",
            "ai-output" | "ai_output" => "AI output",
            other => other,
        }
        .to_owned()
    }

    pub(crate) fn decision_outcome(&self, disposition: &str) -> &'static str {
        match disposition {
            "adopt" => self.session.outcome_adopt,
            "reject" => self.session.outcome_reject,
            "defer" => self.session.outcome_defer,
            _ => self.session.outcome_unknown,
        }
    }

    /// A localized heading for an HTML error page, keyed only by HTTP status.
    pub(crate) fn error_heading(&self, status: StatusCode) -> &'static str {
        match status {
            StatusCode::NOT_FOUND => self.pick("見つかりません", "Not found"),
            StatusCode::METHOD_NOT_ALLOWED => self.pick(
                "この操作には対応していません",
                "This method is not supported",
            ),
            StatusCode::CONFLICT => self.pick("状態が変わりました", "The state changed"),
            StatusCode::SERVICE_UNAVAILABLE => {
                self.pick("一時的に利用できません", "Temporarily unavailable")
            }
            _ => self.pick("ページを表示できません", "The page cannot be shown"),
        }
    }

    /// A localized explanation keyed by the stable problem `code`.
    pub(crate) fn error_summary(&self, code: &str) -> &'static str {
        match code {
            "project_not_found" => self.pick(
                "プロジェクトが見つかりません。起動時に登録されたプロジェクトだけを表示できます。",
                "The project was not found. Only projects registered at startup can be shown.",
            ),
            "creator_session_not_found" => self.pick(
                "セッションが見つかりません。",
                "The session was not found.",
            ),
            "creator_review_state_lost" => self.pick(
                "このprocessのレビュー状態が失われました。セッションを開き直して状態を確認してください。",
                "This process's review state was lost. Reopen the session to check its state.",
            ),
            "ref_conflict" | "stale_base" => self.pick(
                "読み込み中にプロジェクトが変更されました。ページを再読み込みしてください。",
                "The project changed while the page was being built. Reload the page.",
            ),
            "resource_limit" => self.pick(
                "ローカルの処理上限に達しました。",
                "A local processing limit was reached.",
            ),
            "storage_error" | "service_unavailable" => self.pick(
                "ローカルの保存データを読み込めませんでした。時間をおいて再読み込みしてください。",
                "The local storage could not be read. Try reloading later.",
            ),
            "creator_implementation_unrecognized" => self.pick(
                "このセッションは、この版が認識しないSynapseGit（新しい版や未リリースのsource build）で作成されました。データの破損を示すものではありません。作成した版、またはそれ以降の版で開いてください。",
                "This session was recorded by a SynapseGit build this version does not recognize, such as a newer release or an unreleased source build. This does not indicate damaged data. Open it with the recording build or a later release.",
            ),
            "creator_report_invalid" | "fsck_failed" | "oid_mismatch" | "closure_missing"
            | "reference_type_mismatch" | "schema_invalid" => self.pick(
                "記録を検証できませんでした。fsckで状態を確認してください。",
                "The record could not be verified. Check its state with fsck.",
            ),
            "local_request_denied" | "usage_error" | "path_segment_invalid" => self.pick(
                "要求されたページは表示できません。URLを確認してください。",
                "The requested page cannot be shown. Check the URL.",
            ),
            _ => self.pick(
                "ローカルアプリケーションがページを表示できませんでした。",
                "The local application could not show the page.",
            ),
        }
    }

    #[cfg(test)]
    pub(crate) fn entries(&self) -> Vec<(&'static str, &'static str)> {
        [
            self.common.entries(),
            self.index.entries(),
            self.project.entries(),
            self.note.entries(),
            self.session.entries(),
            self.derive.entries(),
            self.reuse.entries(),
            self.presentation.entries(),
            self.error.entries(),
            self.values.entries(),
            self.timeline.entries(),
        ]
        .concat()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(*name, HeaderValue::from_static(value));
        }
        headers
    }

    #[test]
    fn every_catalog_entry_is_translated_and_non_empty() {
        let ja = Locale::Ja.messages().entries();
        let en = Locale::En.messages().entries();
        assert_eq!(ja.len(), en.len());
        for ((ja_name, ja_text), (en_name, en_text)) in ja.iter().zip(&en) {
            assert_eq!(ja_name, en_name);
            // A few entries are intentionally empty suffixes in English word order.
            let optional_suffix = matches!(*ja_name, "confirm_key_after" | "restore_confirm_after");
            assert!(!ja_text.is_empty(), "ja.{ja_name} is empty");
            assert!(
                optional_suffix || !en_text.is_empty(),
                "en.{en_name} is empty"
            );
        }
        for locale in Locale::ALL {
            let messages = locale.messages();
            assert_eq!(messages.locale, locale);
            for disposition in ["adopt", "reject", "defer", "unknown"] {
                assert!(!messages.decision_outcome(disposition).is_empty());
            }
            for status in [
                StatusCode::NOT_FOUND,
                StatusCode::METHOD_NOT_ALLOWED,
                StatusCode::CONFLICT,
                StatusCode::SERVICE_UNAVAILABLE,
                StatusCode::INTERNAL_SERVER_ERROR,
            ] {
                assert!(!messages.error_heading(status).is_empty());
            }
            assert!(!messages.error_summary("unknown_code").is_empty());
        }
    }

    #[test]
    fn english_counts_are_pluralized_and_japanese_counts_are_unchanged() {
        assert_eq!(Locale::Ja.messages().count_sessions(3_usize), "3 件");
        assert_eq!(Locale::En.messages().count_sessions(1_usize), "1 session");
        assert_eq!(Locale::En.messages().count_sessions(0_usize), "0 sessions");
        assert_eq!(Locale::En.messages().count_projects(2_usize), "2 projects");
    }

    #[test]
    fn explicit_selection_requires_one_supported_value() {
        assert_eq!(
            explicit_locale("lang=en"),
            Some((Locale::En, String::new()))
        );
        assert_eq!(
            explicit_locale("a=1&lang=JA&b=%20x"),
            Some((Locale::Ja, "a=1&b=%20x".into()))
        );
        assert_eq!(
            explicit_locale("lang=en&lang=en"),
            Some((Locale::En, String::new()))
        );
        for query in [
            "",
            "a=1",
            "lang",
            "lang=",
            "lang=fr",
            "lang=en-US",
            "lang=en&lang=ja",
            "lang=%65n",
        ] {
            assert_eq!(explicit_locale(query), None, "{query}");
        }
    }

    #[test]
    fn stored_preference_precedes_accept_language_and_default() {
        assert_eq!(resolve_locale(&HeaderMap::new()), Locale::Ja);
        assert_eq!(
            resolve_locale(&headers(&[("accept-language", "en-US,en;q=0.9")])),
            Locale::En
        );
        assert_eq!(
            resolve_locale(&headers(&[
                ("cookie", "other=1; synapse_local_lang=ja"),
                ("accept-language", "en")
            ])),
            Locale::Ja
        );
        assert_eq!(
            resolve_locale(&headers(&[
                ("cookie", "synapse_local_lang=fr"),
                ("accept-language", "en")
            ])),
            Locale::En
        );
        assert_eq!(
            resolve_locale(&headers(&[("cookie", "synapse_local_lang_x=en")])),
            Locale::Ja
        );
    }

    #[test]
    fn accept_language_uses_the_highest_positive_quality_then_order() {
        for (value, expected) in [
            ("fr, en;q=0.4, ja;q=0.8", Some(Locale::Ja)),
            ("ja;q=0, EN-gb;q=0.1", Some(Locale::En)),
            ("en;q=0.5, ja;q=0.5", Some(Locale::En)),
            ("ja-JP, en;q=1.0", Some(Locale::Ja)),
            ("en;q=0.000, ja;q=0.001", Some(Locale::Ja)),
            ("en;q=1.5, ja;q=abc", None),
            ("fr, de;q=0.9, *;q=0.1", None),
            ("", None),
        ] {
            let mut map = HeaderMap::new();
            map.insert(ACCEPT_LANGUAGE, HeaderValue::from_str(value).unwrap());
            assert_eq!(accept_language_locale(&map), expected, "{value}");
        }
    }

    #[test]
    fn only_page_routes_negotiate_a_language() {
        for path in ["/", "/projects/demo", "/missing", "//example.test", "/api"] {
            assert!(is_page_path(path), "{path}");
        }
        for path in ["/api/v1/health", "/api/v1", "/assets/app.js"] {
            assert!(!is_page_path(path), "{path}");
        }
    }
}
