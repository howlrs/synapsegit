use crate::model::{
    PresentedText, PublicProjection, PublicSession, PublicationLocale, ValueOrigin,
};
use std::fmt::Write as _;

pub(crate) fn render_story(projection: &PublicProjection, locale: PublicationLocale) -> String {
    let mut output = String::new();
    let title = markdown_inline(presented(locale, &projection.presentation.title));
    writeln!(output, "# {title}\n").expect("writing to String cannot fail");
    writeln!(
        output,
        "{} · {} `{}` · {} `0`\n",
        key(locale, Key::ProviderNeutralView),
        key(locale, Key::Visibility),
        projection.publication.visibility.as_str(),
        key(locale, Key::NetworkOperations)
    )
    .expect("writing to String cannot fail");
    write_markdown_paragraph(
        &mut output,
        &summary_value(locale, &projection.presentation.summary),
    );
    if let Some(creator) = &projection.presentation.creator_display_name {
        writeln!(
            output,
            "\n{}: **{}** ({})",
            key(locale, Key::CreatorLabel),
            markdown_inline(&creator.value),
            key(locale, Key::AuthorSupplied)
        )
        .expect("writing to String cannot fail");
    }
    if let Some(agent) = &projection.presentation.proposal_agent_display_name {
        writeln!(
            output,
            "{}: **{}** ({})",
            key(locale, Key::ProposalAgentLabel),
            markdown_inline(&agent.value),
            key(locale, Key::AuthorSupplied)
        )
        .expect("writing to String cannot fail");
    }

    writeln!(output, "\n## {}\n", key(locale, Key::ReadingHistory))
        .expect("writing to String cannot fail");
    writeln!(output, "{}\n", key(locale, Key::MarkdownProofLimit))
        .expect("writing to String cannot fail");

    if projection.sessions.is_empty() {
        writeln!(output, "{}\n", key(locale, Key::NoCompleteSession))
            .expect("writing to String cannot fail");
    }
    for session in &projection.sessions {
        render_story_session(&mut output, session, locale);
    }

    if !projection.incomplete_sessions.is_empty() {
        writeln!(output, "## {}\n", key(locale, Key::IncompleteSessions))
            .expect("writing to String cannot fail");
        writeln!(output, "{}\n", key(locale, Key::IncompleteExplanation))
            .expect("writing to String cannot fail");
        for session in &projection.incomplete_sessions {
            writeln!(
                output,
                "- `{}` — {}: `{}`, {}: `{}`",
                markdown_code(&session.session),
                key(locale, Key::ProposalPresent),
                session.proposal_present,
                key(locale, Key::DecisionPresent),
                session.decision_present
            )
            .expect("writing to String cannot fail");
        }
        output.push('\n');
    }

    writeln!(output, "## {}\n", key(locale, Key::DisclosureAndLimits))
        .expect("writing to String cannot fail");
    for limitation in &projection.limitations {
        writeln!(
            output,
            "- **{}** — {}",
            markdown_inline(&limitation.code),
            markdown_inline(limitation_message(locale, limitation.code.as_str()))
        )
        .expect("writing to String cannot fail");
    }
    writeln!(output, "\n{}", key(locale, Key::MachineSemanticsMarkdown))
        .expect("writing to String cannot fail");
    output
}

fn render_story_session(output: &mut String, session: &PublicSession, locale: PublicationLocale) {
    writeln!(
        output,
        "## {}\n",
        markdown_inline(presented(locale, &session.title))
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "{}: `{}`\n",
        key(locale, Key::Session),
        markdown_code(&session.session)
    )
    .expect("writing to String cannot fail");

    writeln!(output, "### {}\n", key(locale, Key::WorkHistory))
        .expect("writing to String cannot fail");
    writeln!(
        output,
        "| {} | {} | {} | {} |",
        key(locale, Key::Role),
        key(locale, Key::PublicCaption),
        key(locale, Key::VerifiedSourceOid),
        key(locale, Key::Rendering)
    )
    .expect("writing to String cannot fail");
    writeln!(output, "|---|---|---|---|").expect("writing to String cannot fail");
    for artifact in &session.history {
        writeln!(
            output,
            "| {} | {} | `{}` | {} |",
            markdown_table(role_label(locale, artifact.role.label())),
            markdown_table(presented(locale, &artifact.caption)),
            markdown_code(&artifact.oid),
            markdown_table(key(locale, Key::AssetBytesOmitted))
        )
        .expect("writing to String cannot fail");
    }
    output.push('\n');

    writeln!(output, "### {}\n", key(locale, Key::ProposalAndDecision))
        .expect("writing to String cannot fail");
    writeln!(
        output,
        "- {}: {}",
        key(locale, Key::ProposalAttribution),
        markdown_inline(attribution_scope(locale, session))
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- {}: **{}**",
        key(locale, Key::HumanDisposition),
        markdown_inline(&session.human_decision.disposition)
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- {}: **{}**",
        key(locale, Key::SelectedRole),
        markdown_inline(role_label(
            locale,
            session.human_decision.selected_artifact.label()
        ))
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- {}: `{}`",
        key(locale, Key::ProposalRetained),
        session.proposal.retained_when_unselected
    )
    .expect("writing to String cannot fail");
    if let Some(note) = &session.human_decision.public_decision_note {
        writeln!(
            output,
            "\n{} ({})\n",
            key(locale, Key::PublicDecisionNote),
            key(locale, Key::AuthorSupplied)
        )
        .expect("writing to String cannot fail");
        write_markdown_quote(output, &note.value);
        output.push('\n');
    } else {
        writeln!(output, "\n{}\n", key(locale, Key::NoPublicDecisionNote))
            .expect("writing to String cannot fail");
    }

    if let Some(comparison) = &session.comparison {
        writeln!(output, "### {}\n", key(locale, Key::Evidence))
            .expect("writing to String cannot fail");
        match locale {
            PublicationLocale::En => writeln!(
                output,
                "The current comparison reports **{}** with comparability **{}**. {}\n",
                markdown_inline(&comparison.outcome),
                markdown_inline(&comparison.comparability),
                markdown_inline(key(locale, Key::ComparisonInterpretationLimit))
            ),
            PublicationLocale::Ja => writeln!(
                output,
                "{} **{}**、{} **{}**。{}\n",
                key(locale, Key::Outcome),
                markdown_inline(&comparison.outcome),
                key(locale, Key::Comparability),
                markdown_inline(&comparison.comparability),
                markdown_inline(key(locale, Key::ComparisonInterpretationLimit))
            ),
        }
        .expect("writing to String cannot fail");
    }

    writeln!(output, "### {}\n", key(locale, Key::TechnicalProvenance))
        .expect("writing to String cannot fail");
    writeln!(
        output,
        "- {}: `{}`\n- {}: `{}`\n- {}: `{}`\n- {}: `{}`\n- {}: `{}`\n- {}: `{}`\n- {}: `{}`\n",
        key(locale, Key::ProposalRef),
        markdown_code(&session.provenance.proposal_ref),
        key(locale, Key::DecisionRef),
        markdown_code(&session.provenance.decision_ref),
        key(locale, Key::BaseHead),
        markdown_code(&session.provenance.base_head),
        key(locale, Key::ProposalHead),
        markdown_code(&session.provenance.proposal_head),
        key(locale, Key::DecisionHead),
        markdown_code(&session.provenance.decision_head),
        key(locale, Key::ProjectionFingerprint),
        markdown_code(&session.provenance.projection_source_fingerprint),
        key(locale, Key::ObjectsVerified),
        session.provenance.fsck_objects_verified
    )
    .expect("writing to String cannot fail");
}

pub(crate) fn render_html(projection: &PublicProjection, locale: PublicationLocale) -> String {
    let mut output = String::new();
    write!(
        output,
        "<!doctype html>\n<html lang=\"{}\"><head><meta charset=\"utf-8\">\n",
        locale.as_str()
    )
    .expect("writing to String cannot fail");
    output.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n");
    output.push_str("<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src 'self'; base-uri 'none'; form-action 'none'\">\n");
    writeln!(
        output,
        "<title>{}</title>",
        html(presented(locale, &projection.presentation.title))
    )
    .expect("writing to String cannot fail");
    output.push_str(STYLE);
    output.push_str("</head><body><main>\n");
    writeln!(
        output,
        "<header><p class=\"eyebrow\">{}</p><h1>{}</h1><p class=\"lead\">{}</p><div class=\"badges\"><span>{}</span><span>{}: 0</span></div>",
        key(locale, Key::ProviderNeutralView),
        html(presented(locale, &projection.presentation.title)),
        html(&summary_value(locale, &projection.presentation.summary)),
        html(projection.publication.visibility.as_str()), key(locale, Key::Network)
    )
    .expect("writing to String cannot fail");
    if let Some(creator) = &projection.presentation.creator_display_name {
        write!(
            output,
            "<p class=\"byline\">{}: <strong>{}</strong> <span class=\"muted\">({})</span></p>",
            key(locale, Key::CreatorLabel),
            html(&creator.value),
            key(locale, Key::AuthorSupplied)
        )
        .expect("writing to String cannot fail");
    }
    if let Some(agent) = &projection.presentation.proposal_agent_display_name {
        write!(
            output,
            "<p class=\"byline\">{}: <strong>{}</strong> <span class=\"muted\">({})</span></p>",
            key(locale, Key::ProposalAgentLabel),
            html(&agent.value),
            key(locale, Key::AuthorSupplied)
        )
        .expect("writing to String cannot fail");
    }
    output.push_str("</header>\n");
    writeln!(
        output,
        "<section class=\"notice\"><h2>{}</h2><p>{}</p></section>",
        key(locale, Key::HowToRead),
        key(locale, Key::HtmlProofLimit)
    )
    .expect("writing to String cannot fail");

    if projection.sessions.is_empty() {
        writeln!(
            output,
            "<section><h2>{}</h2><p>{}</p></section>",
            key(locale, Key::NoCompleteSessionHeading),
            key(locale, Key::NoCompleteSession)
        )
        .expect("writing to String cannot fail");
    }
    for session in &projection.sessions {
        render_html_session(&mut output, session, locale);
    }

    if !projection.incomplete_sessions.is_empty() {
        write!(
            output,
            "<section><h2>{}</h2><ul>",
            key(locale, Key::IncompleteSessions)
        )
        .expect("writing to String cannot fail");
        for session in &projection.incomplete_sessions {
            write!(
                output,
                "<li><code>{}</code> — {}: {}, {}: {}</li>",
                html(&session.session),
                key(locale, Key::ProposalPresent),
                session.proposal_present,
                key(locale, Key::DecisionPresent),
                session.decision_present
            )
            .expect("writing to String cannot fail");
        }
        output.push_str("</ul></section>\n");
    }

    write!(
        output,
        "<section><h2>{}</h2><ul>",
        key(locale, Key::DisclosureAndLimits)
    )
    .expect("writing to String cannot fail");
    for limitation in &projection.limitations {
        write!(
            output,
            "<li><strong>{}</strong> — {}</li>",
            html(&limitation.code),
            html(limitation_message(locale, limitation.code.as_str()))
        )
        .expect("writing to String cannot fail");
    }
    writeln!(
        output,
        "</ul><p>{}</p></section>",
        key(locale, Key::MachineSemanticsHtml)
    )
    .expect("writing to String cannot fail");
    output.push_str("</main></body></html>\n");
    output
}

fn render_html_session(output: &mut String, session: &PublicSession, locale: PublicationLocale) {
    write!(
        output,
        "<article><p class=\"eyebrow\">{} <code>{}</code></p><h2>{}</h2><div class=\"artifact-grid\">", key(locale, Key::Session),
        html(&session.session),
        html(presented(locale, &session.title))
    )
    .expect("writing to String cannot fail");
    for artifact in &session.history {
        write!(
            output,
            "<section class=\"artifact\"><p class=\"role\">{}</p><h3>{}</h3><div class=\"placeholder\" aria-label=\"{}\">{}</div><p>{}</p><code class=\"oid\">{}</code></section>",
            html(role_label(locale, artifact.role.label())),
            html(presented(locale, &artifact.caption)),
            key(locale, Key::AssetBytesOmitted), key(locale, Key::AssetBytesOmitted),
            key(locale, Key::AssetBytesOmitted),
            html(&artifact.oid)
        )
        .expect("writing to String cannot fail");
    }
    output.push_str("</div>");
    let selected = session.human_decision.selected_artifact.label();
    write!(
        output,
        "<section class=\"decision\"><p class=\"eyebrow\">{}</p><h3>{}</h3><p>{}: <strong>{}</strong></p><p>{}: <strong>{}</strong></p>", key(locale, Key::HumanDecision),
        html(&session.human_decision.disposition),
        key(locale, Key::SelectedRole), html(role_label(locale, selected)),
        key(locale, Key::ProposalRetained),
        session.proposal.retained_when_unselected
    )
    .expect("writing to String cannot fail");
    if let Some(note) = &session.human_decision.public_decision_note {
        write!(
            output,
            "<blockquote><p>{}</p><footer>{} ({})</footer></blockquote>",
            html_with_breaks(&note.value),
            key(locale, Key::PublicDecisionNote),
            key(locale, Key::AuthorSupplied)
        )
        .expect("writing to String cannot fail");
    } else {
        write!(
            output,
            "<p class=\"muted\">{}</p>",
            key(locale, Key::NoPublicDecisionNote)
        )
        .expect("writing to String cannot fail");
    }
    output.push_str("</section>");
    if let Some(comparison) = &session.comparison {
        write!(
            output,
            "<section><h3>{}</h3><p>{}: <strong>{}</strong>; {}: <strong>{}</strong>.</p><p>{}</p></section>", key(locale, Key::Evidence), key(locale, Key::Outcome),
            html(&comparison.outcome),
            key(locale, Key::Comparability),
            html(&comparison.comparability),
            html(key(locale, Key::ComparisonInterpretationLimit))
        )
        .expect("writing to String cannot fail");
    }
    write!(
        output,
        "<details><summary>{}</summary><dl><dt>{}</dt><dd><code>{}</code></dd><dt>{}</dt><dd><code>{}</code></dd><dt>{}</dt><dd><code>{}</code></dd><dt>{}</dt><dd><code>{}</code></dd></dl></details></article>", key(locale, Key::TechnicalProvenance), key(locale, Key::ProposalRef),
        html(&session.provenance.proposal_ref),
        key(locale, Key::DecisionRef),
        html(&session.provenance.decision_ref),
        key(locale, Key::DecisionHead),
        html(&session.provenance.decision_head),
        key(locale, Key::ProjectionFingerprint),
        html(&session.provenance.projection_source_fingerprint)
    )
    .expect("writing to String cannot fail");
}

#[derive(Clone, Copy)]
enum Key {
    ProviderNeutralView,
    Visibility,
    NetworkOperations,
    Network,
    ReadingHistory,
    MarkdownProofLimit,
    DisclosureAndLimits,
    HowToRead,
    HtmlProofLimit,
    CreatorLabel,
    ProposalAgentLabel,
    AuthorSupplied,
    NoCompleteSession,
    IncompleteSessions,
    IncompleteExplanation,
    ProposalPresent,
    DecisionPresent,
    MachineSemanticsMarkdown,
    Session,
    WorkHistory,
    Role,
    PublicCaption,
    VerifiedSourceOid,
    Rendering,
    ProposalAndDecision,
    ProposalAttribution,
    HumanDisposition,
    SelectedRole,
    ProposalRetained,
    PublicDecisionNote,
    NoPublicDecisionNote,
    Evidence,
    Outcome,
    Comparability,
    TechnicalProvenance,
    ProposalRef,
    DecisionRef,
    BaseHead,
    ProposalHead,
    DecisionHead,
    ProjectionFingerprint,
    ObjectsVerified,
    NoCompleteSessionHeading,
    MachineSemanticsHtml,
    AssetBytesOmitted,
    HumanDecision,
    ComparisonInterpretationLimit,
}

#[cfg(test)]
const ALL_KEYS: &[Key] = &[
    Key::ProviderNeutralView,
    Key::Visibility,
    Key::NetworkOperations,
    Key::Network,
    Key::ReadingHistory,
    Key::MarkdownProofLimit,
    Key::DisclosureAndLimits,
    Key::HowToRead,
    Key::HtmlProofLimit,
    Key::CreatorLabel,
    Key::ProposalAgentLabel,
    Key::AuthorSupplied,
    Key::NoCompleteSession,
    Key::IncompleteSessions,
    Key::IncompleteExplanation,
    Key::ProposalPresent,
    Key::DecisionPresent,
    Key::MachineSemanticsMarkdown,
    Key::Session,
    Key::WorkHistory,
    Key::Role,
    Key::PublicCaption,
    Key::VerifiedSourceOid,
    Key::Rendering,
    Key::ProposalAndDecision,
    Key::ProposalAttribution,
    Key::HumanDisposition,
    Key::SelectedRole,
    Key::ProposalRetained,
    Key::PublicDecisionNote,
    Key::NoPublicDecisionNote,
    Key::Evidence,
    Key::Outcome,
    Key::Comparability,
    Key::TechnicalProvenance,
    Key::ProposalRef,
    Key::DecisionRef,
    Key::BaseHead,
    Key::ProposalHead,
    Key::DecisionHead,
    Key::ProjectionFingerprint,
    Key::ObjectsVerified,
    Key::NoCompleteSessionHeading,
    Key::MachineSemanticsHtml,
    Key::AssetBytesOmitted,
    Key::HumanDecision,
    Key::ComparisonInterpretationLimit,
];

fn key(locale: PublicationLocale, value: Key) -> &'static str {
    match (locale, value) {
        (PublicationLocale::En, Key::ComparisonInterpretationLimit) => {
            crate::COMPARISON_INTERPRETATION_LIMIT
        }
        (PublicationLocale::Ja, Key::ComparisonInterpretationLimit) => {
            "primary Blobのバイトだけを比較します。ピクセル、意味、物理的な変化の解析ではありません。"
        }
        (PublicationLocale::En, Key::ProviderNeutralView) => {
            "SynapseGit provider-neutral publication view"
        }
        (PublicationLocale::Ja, Key::ProviderNeutralView) => {
            "SynapseGit プロバイダー中立の公開ビュー"
        }
        (PublicationLocale::En, Key::Visibility) => "visibility",
        (PublicationLocale::Ja, Key::Visibility) => "公開範囲",
        (PublicationLocale::En, Key::NetworkOperations) => "network operations",
        (PublicationLocale::Ja, Key::NetworkOperations) => "ネットワーク操作",
        (PublicationLocale::En, Key::Network) => "network",
        (PublicationLocale::Ja, Key::Network) => "ネットワーク",
        (PublicationLocale::En, Key::ReadingHistory) => "Reading this history",
        (PublicationLocale::Ja, Key::ReadingHistory) => "この履歴の読み方",
        (PublicationLocale::En, Key::MarkdownProofLimit) => {
            "This view separates the original, the recorded current state, the AI-attributed proposal, and the Human decision. OIDs verify byte identity in the source repository; they do not prove authorship, truth, copyright, permission, or physical change."
        }
        (PublicationLocale::Ja, Key::MarkdownProofLimit) => {
            "このビューは、Original、記録されたCurrent、AIに帰属する提案、人間の判断を分けて示します。OIDはソースリポジトリ内のバイト同一性を検証しますが、著者性、真実、著作権、許可、物理的な変化を証明しません。"
        }
        (PublicationLocale::En, Key::DisclosureAndLimits) => "Disclosure and limits",
        (PublicationLocale::Ja, Key::DisclosureAndLimits) => "開示と限界",
        (PublicationLocale::En, Key::HowToRead) => "How to read this view",
        (PublicationLocale::Ja, Key::HowToRead) => "このビューの読み方",
        (PublicationLocale::En, Key::HtmlProofLimit) => {
            "Original, current state, AI-attributed proposal, and Human decision are distinct roles. OIDs verify source byte identity only; they do not prove authorship, truth, rights, permission, or physical change."
        }
        (PublicationLocale::Ja, Key::HtmlProofLimit) => {
            "Original、Current、AIに帰属する提案、人間の判断は別の役割です。OIDはソースのバイト同一性だけを検証し、著者性、真実、権利、許可、物理的な変化を証明しません。"
        }
        (PublicationLocale::En, Key::CreatorLabel) => "Creator label",
        (PublicationLocale::Ja, Key::CreatorLabel) => "制作者表示名",
        (PublicationLocale::En, Key::ProposalAgentLabel) => "Proposal agent label",
        (PublicationLocale::Ja, Key::ProposalAgentLabel) => "提案エージェント表示名",
        (PublicationLocale::En, Key::AuthorSupplied) => "author supplied",
        (PublicationLocale::Ja, Key::AuthorSupplied) => "著者提供",
        (PublicationLocale::En, Key::NoCompleteSession) => {
            "No complete creator session was available in the selected source snapshot."
        }
        (PublicationLocale::Ja, Key::NoCompleteSession) => {
            "選択したソーススナップショットには完了したCreator sessionがありません。"
        }
        (PublicationLocale::En, Key::NoCompleteSessionHeading) => "No complete session",
        (PublicationLocale::Ja, Key::NoCompleteSessionHeading) => "完了したsessionはありません",
        (PublicationLocale::En, Key::IncompleteSessions) => "Incomplete sessions",
        (PublicationLocale::Ja, Key::IncompleteSessions) => "未完了のsession",
        (PublicationLocale::En, Key::IncompleteExplanation) => {
            "These retained Ref shapes were not promoted into complete stories:"
        }
        (PublicationLocale::Ja, Key::IncompleteExplanation) => {
            "保持された次のRef形状は完了した履歴として昇格されませんでした："
        }
        (PublicationLocale::En, Key::ProposalPresent) => "proposal present",
        (PublicationLocale::Ja, Key::ProposalPresent) => "proposalあり",
        (PublicationLocale::En, Key::DecisionPresent) => "decision present",
        (PublicationLocale::Ja, Key::DecisionPresent) => "decisionあり",
        (PublicationLocale::En, Key::MachineSemanticsMarkdown) => {
            "Machine-readable semantics are available in [`projection.json`](./projection.json). Machine readability does not grant training permission; this bundle declares `training_use_policy=prohibited`."
        }
        (PublicationLocale::Ja, Key::MachineSemanticsMarkdown) => {
            "機械可読な意味表現は[`projection.json`](./projection.json)にあります。機械可読であっても学習の許可は与えられません。このbundleは`training_use_policy=prohibited`を宣言します。"
        }
        (PublicationLocale::En, Key::MachineSemanticsHtml) => {
            "Machine-readable semantics: <a href=\"projection.json\">projection.json</a>. Machine readability does not grant training permission."
        }
        (PublicationLocale::Ja, Key::MachineSemanticsHtml) => {
            "機械可読な意味表現：projection.json。機械可読であっても学習の許可は与えられません。"
        }
        (PublicationLocale::En, Key::AssetBytesOmitted) => "Asset bytes omitted by policy",
        (PublicationLocale::Ja, Key::AssetBytesOmitted) => "ポリシーによりassetのバイトは省略",
        (PublicationLocale::En, Key::HumanDecision) => "Human decision",
        (PublicationLocale::Ja, Key::HumanDecision) => "人間の判断",
        (PublicationLocale::En, Key::Session) => "Session",
        (PublicationLocale::Ja, Key::Session) => "Session",
        (PublicationLocale::En, Key::WorkHistory) => "Work history",
        (PublicationLocale::Ja, Key::WorkHistory) => "作業履歴",
        (PublicationLocale::En, Key::Role) => "Role",
        (PublicationLocale::Ja, Key::Role) => "役割",
        (PublicationLocale::En, Key::PublicCaption) => "Public caption",
        (PublicationLocale::Ja, Key::PublicCaption) => "公開caption",
        (PublicationLocale::En, Key::VerifiedSourceOid) => "Verified source OID",
        (PublicationLocale::Ja, Key::VerifiedSourceOid) => "検証済みソースOID",
        (PublicationLocale::En, Key::Rendering) => "Rendering",
        (PublicationLocale::Ja, Key::Rendering) => "表示",
        (PublicationLocale::En, Key::ProposalAndDecision) => "Proposal and Human decision",
        (PublicationLocale::Ja, Key::ProposalAndDecision) => "提案と人間の判断",
        (PublicationLocale::En, Key::ProposalAttribution) => "Proposal attribution",
        (PublicationLocale::Ja, Key::ProposalAttribution) => "提案の帰属",
        (PublicationLocale::En, Key::HumanDisposition) => "Human disposition",
        (PublicationLocale::Ja, Key::HumanDisposition) => "人間の判断",
        (PublicationLocale::En, Key::SelectedRole) => "Selected role",
        (PublicationLocale::Ja, Key::SelectedRole) => "選択された役割",
        (PublicationLocale::En, Key::ProposalRetained) => {
            "Proposal retained in history even when unselected"
        }
        (PublicationLocale::Ja, Key::ProposalRetained) => "選択されなくても提案を履歴に保持",
        (PublicationLocale::En, Key::PublicDecisionNote) => "Public decision note",
        (PublicationLocale::Ja, Key::PublicDecisionNote) => "公開判断メモ",
        (PublicationLocale::En, Key::NoPublicDecisionNote) => {
            "No public decision note was supplied. The source rationale remains redacted because its stored visibility is private and its training-use policy is prohibited."
        }
        (PublicationLocale::Ja, Key::NoPublicDecisionNote) => {
            "公開判断メモは提供されていません。保存された理由はprivateであり学習利用が禁止されているため非公開です。"
        }
        (PublicationLocale::En, Key::Evidence) => "Evidence",
        (PublicationLocale::Ja, Key::Evidence) => "根拠",
        (PublicationLocale::En, Key::Outcome) => "Outcome",
        (PublicationLocale::Ja, Key::Outcome) => "結果",
        (PublicationLocale::En, Key::Comparability) => "comparability",
        (PublicationLocale::Ja, Key::Comparability) => "比較可能性",
        (PublicationLocale::En, Key::TechnicalProvenance) => "Technical provenance",
        (PublicationLocale::Ja, Key::TechnicalProvenance) => "技術的来歴",
        (PublicationLocale::En, Key::ProposalRef) => "Proposal Ref",
        (PublicationLocale::Ja, Key::ProposalRef) => "Proposal Ref",
        (PublicationLocale::En, Key::DecisionRef) => "Decision Ref",
        (PublicationLocale::Ja, Key::DecisionRef) => "Decision Ref",
        (PublicationLocale::En, Key::BaseHead) => "Base head",
        (PublicationLocale::Ja, Key::BaseHead) => "Base head",
        (PublicationLocale::En, Key::ProposalHead) => "Proposal head",
        (PublicationLocale::Ja, Key::ProposalHead) => "Proposal head",
        (PublicationLocale::En, Key::DecisionHead) => "Decision head",
        (PublicationLocale::Ja, Key::DecisionHead) => "Decision head",
        (PublicationLocale::En, Key::ProjectionFingerprint) => "Projection fingerprint",
        (PublicationLocale::Ja, Key::ProjectionFingerprint) => "Projection fingerprint",
        (PublicationLocale::En, Key::ObjectsVerified) => "Objects verified by session fsck",
        (PublicationLocale::Ja, Key::ObjectsVerified) => "session fsckで検証したobject",
    }
}

fn role_label(locale: PublicationLocale, role: &str) -> &str {
    match (locale, role) {
        (PublicationLocale::Ja, "Original") => "Original",
        (PublicationLocale::Ja, "Current") => "Current",
        (PublicationLocale::Ja, "AI-attributed proposal") => "AIに帰属する提案",
        _ => role,
    }
}

fn presented(locale: PublicationLocale, value: &PresentedText) -> &str {
    if value.origin != ValueOrigin::DerivedSummary || locale == PublicationLocale::En {
        return &value.value;
    }
    match value.value.as_str() {
        "SynapseGit creative history" => "SynapseGit 制作履歴",
        "Recorded original source" => "記録されたOriginal source",
        "Recorded current state" => "記録されたCurrent state",
        "AI-attributed proposal" => "AIに帰属する提案",
        value if value.starts_with("Session ") => "Session（記録済み識別子）",
        _ => &value.value,
    }
}

fn attribution_scope(locale: PublicationLocale, session: &PublicSession) -> &str {
    if locale == PublicationLocale::Ja
        && session.proposal.attribution_scope_origin == ValueOrigin::DerivedSummary
        && session.proposal.attribution_scope
            == "Caller-supplied output recorded by the workflow as AI-attributed; no model invocation is independently verified"
    {
        "呼び出し元提供のoutputはworkflowによりAIへ帰属します。model invocationは独立に検証されていません。"
    } else {
        &session.proposal.attribution_scope
    }
}

fn summary_value(locale: PublicationLocale, value: &PresentedText) -> String {
    if locale == PublicationLocale::Ja
        && value.origin == ValueOrigin::DerivedSummary
        && value.value.starts_with("A reviewable history of ")
    {
        let count = value.value.split_whitespace().nth(5).unwrap_or("0");
        format!(
            "完了したCreator sessionは{count}件です。AIに帰属する提案と人間の判断を保持し、raw source assetを公開しない確認可能な制作履歴です。"
        )
    } else {
        value.value.clone()
    }
}

fn limitation_message(locale: PublicationLocale, code: &str) -> &'static str {
    match (locale, code) {
        (PublicationLocale::En, "byte_identity_only") => {
            "The recorded OID verifies byte identity only; it does not prove authorship, truth, rights, permission, or physical change."
        }
        (PublicationLocale::Ja, "byte_identity_only") => {
            "記録されたOIDはバイト同一性だけを検証し、著者性、真実、権利、許可、物理的な変化を証明しません。"
        }
        (PublicationLocale::En, "raw_assets_omitted") => {
            "Original, current, and proposal bytes are omitted by default to avoid leaking metadata, active content, or unrelated private material."
        }
        (PublicationLocale::Ja, "raw_assets_omitted") => {
            "metadata、active content、無関係なprivate materialの漏えいを避けるため、Original、Current、Proposalのバイトは既定で省略されます。"
        }
        (PublicationLocale::En, "private_rationale_redacted") => {
            "The source CreatorReport does not expose a verified feedback visibility policy. Its rationale is therefore withheld; only a separately supplied public decision note may appear."
        }
        (PublicationLocale::Ja, "private_rationale_redacted") => {
            "source CreatorReportには検証済みのfeedback公開範囲がありません。そのため理由は非公開であり、別途提供された公開判断メモだけが表示されます。"
        }
        (PublicationLocale::En, "training_prohibited") => {
            "Machine-readable output is provided for inspection and interoperability, not as permission to train on the content."
        }
        (PublicationLocale::Ja, "training_prohibited") => {
            "機械可読な出力は確認と相互運用のためのものであり、contentの学習許可ではありません。"
        }
        (PublicationLocale::En, "local_bundle_only") => {
            "This export performs no Git, GitHub, Synapse service, upload, or other network operation and contains no remote publication receipt."
        }
        (PublicationLocale::Ja, "local_bundle_only") => {
            "このexportはnetwork operationを行わず、remote publication receiptを含みません。"
        }
        (PublicationLocale::En, "artifact_preview_unavailable") => {
            "This first safe renderer identifies artifacts by role and OID but does not include their raw bytes or thumbnails."
        }
        (PublicationLocale::Ja, "artifact_preview_unavailable") => {
            "このsafe rendererはartifactをroleとOIDで示しますが、raw bytesやthumbnailは含めません。"
        }
        (PublicationLocale::En, "attribution_is_scoped") => {
            "The proposal is AI-attributed by the recorded workflow. This bundle does not verify that a model generated the supplied bytes or identify a model invocation."
        }
        (PublicationLocale::Ja, "attribution_is_scoped") => {
            "Proposalは記録されたworkflowによりAIへ帰属します。このbundleはmodelが提供されたバイトを生成したことやmodel invocationを検証しません。"
        }
        (PublicationLocale::En, "identifier_correlation") => {
            "Artifact OIDs and technical Ref/Commit identifiers can correlate this view with another copy of the same history; review them before external publication."
        }
        (PublicationLocale::Ja, "identifier_correlation") => {
            "Artifact OIDと技術的なRef/Commit identifierは、このビューを同じ履歴の別copyと対応付けられます。外部公開前に確認してください。"
        }
        (PublicationLocale::En, "bundle_not_signed") => {
            "Checksums detect accidental bundle damage but are not an identity signature or proof of who published the bundle."
        }
        (PublicationLocale::Ja, "bundle_not_signed") => {
            "checksumは偶発的なbundle破損を検出しますが、identity signatureや公開者の証明ではありません。"
        }
        (PublicationLocale::En, "projection_fingerprint_unavailable") => {
            "No complete creator report was available, so only the deterministic Ref snapshot digest is present."
        }
        (PublicationLocale::Ja, "projection_fingerprint_unavailable") => {
            "完了したcreator reportがないため、deterministic Ref snapshot digestだけが存在します。"
        }
        (PublicationLocale::En, "source_rationale_not_public") => {
            "The recorded source rationale was not copied. A public decision note, when present, is separate author-supplied text."
        }
        (PublicationLocale::Ja, "source_rationale_not_public") => {
            "記録済みsource rationaleはcopyされません。公開判断メモがある場合も別のauthor-supplied textです。"
        }
        (PublicationLocale::En, _) => {
            "This publication has the limits stated by its safe local profile."
        }
        (PublicationLocale::Ja, _) => "この公開には安全なlocal profileで定められた限界があります。",
    }
}

fn markdown_inline(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' | '`' | '*' | '_' | '{' | '}' | '[' | ']' | '<' | '>' | '(' | ')' | '#' | '+'
            | '-' | '!' | '|' => {
                escaped.push('\\');
                escaped.push(character);
            }
            '\n' | '\r' => escaped.push(' '),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn markdown_code(value: &str) -> String {
    value.replace('`', "\\`").replace(['\n', '\r'], " ")
}

fn markdown_table(value: &str) -> String {
    markdown_inline(value).replace('\n', "<br>")
}

fn write_markdown_paragraph(output: &mut String, value: &str) {
    for (index, line) in value.lines().enumerate() {
        if index > 0 {
            output.push_str("  \n");
        }
        output.push_str(&markdown_inline(line));
    }
    output.push('\n');
}

fn write_markdown_quote(output: &mut String, value: &str) {
    if value.is_empty() {
        output.push_str("> \n");
        return;
    }
    for line in value.lines() {
        writeln!(output, "> {}", markdown_inline(line)).expect("writing to String cannot fail");
    }
}

fn html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn html_with_breaks(value: &str) -> String {
    html(value).replace('\n', "<br>")
}

const STYLE: &str = r#"<style>
:root{color-scheme:light;--ink:#172019;--muted:#5d665f;--paper:#f5f2e9;--panel:#fffdf7;--line:#d9d3c5;--accent:#276749;--proposal:#e9f3ec;--decision:#fff3cd}*{box-sizing:border-box}body{margin:0;background:var(--paper);color:var(--ink);font:16px/1.6 ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}main{width:min(1120px,calc(100% - 2rem));margin:0 auto;padding:4rem 0 6rem}header,article,section.notice,main>section{background:var(--panel);border:1px solid var(--line);border-radius:18px;padding:clamp(1.25rem,3vw,2.5rem);margin-bottom:1.5rem;box-shadow:0 10px 28px rgb(23 32 25/.06)}h1{font-size:clamp(2.2rem,6vw,4.8rem);line-height:1.02;max-width:16ch;margin:.25rem 0 1rem}h2{font-size:clamp(1.5rem,3vw,2.4rem);line-height:1.15}.lead{max-width:70ch;font-size:1.15rem}.eyebrow,.role{text-transform:uppercase;letter-spacing:.12em;font-size:.75rem;font-weight:750;color:var(--accent)}.badges{display:flex;flex-wrap:wrap;gap:.5rem;margin-top:1.5rem}.badges span{border:1px solid var(--line);border-radius:999px;padding:.25rem .7rem;background:#fff}.artifact-grid{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:1rem;margin:1.5rem 0}.artifact{border:1px solid var(--line);border-radius:14px;padding:1rem;background:#fff}.placeholder{min-height:150px;display:grid;place-items:center;text-align:center;border-radius:10px;background:linear-gradient(135deg,#e8e2d4,#f7f4ec);color:var(--muted);padding:1rem}.oid,code{overflow-wrap:anywhere}.decision{background:var(--decision);border-radius:14px;padding:1.25rem;margin:1rem 0}blockquote{margin:1rem 0;padding:1rem 1.25rem;border-left:4px solid var(--accent);background:#fff}blockquote footer,.muted{color:var(--muted)}details{margin-top:1rem;border-top:1px solid var(--line);padding-top:1rem}dt{font-weight:700;margin-top:.75rem}dd{margin-left:0}a{color:var(--accent)}@media(max-width:760px){main{padding-top:1rem}.artifact-grid{grid-template-columns:1fr}}
</style>
"#;

#[cfg(test)]
mod tests {
    use super::{ALL_KEYS, html, key, markdown_inline};
    use crate::model::PublicationLocale;

    #[test]
    fn escapes_active_markup() {
        assert_eq!(html("<script>&\"'"), "&lt;script&gt;&amp;&quot;&#39;");
        assert_eq!(markdown_inline("<script>|x"), "\\<script\\>\\|x");
    }

    #[test]
    fn every_fixed_key_has_nonempty_text_in_both_locales() {
        for value in ALL_KEYS {
            assert!(!key(PublicationLocale::En, *value).is_empty());
            assert!(!key(PublicationLocale::Ja, *value).is_empty());
        }
    }
}
