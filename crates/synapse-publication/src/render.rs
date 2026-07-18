use crate::model::{PublicProjection, PublicSession};
use std::fmt::Write as _;

pub(crate) fn render_story(projection: &PublicProjection) -> String {
    let mut output = String::new();
    let title = markdown_inline(&projection.presentation.title.value);
    writeln!(output, "# {title}\n").expect("writing to String cannot fail");
    writeln!(
        output,
        "> SynapseGit provider-neutral publication view · visibility `{}` · network operations `0`\n",
        projection.publication.visibility.as_str()
    )
    .expect("writing to String cannot fail");
    write_markdown_paragraph(&mut output, &projection.presentation.summary.value);
    if let Some(creator) = &projection.presentation.creator_display_name {
        writeln!(
            output,
            "\nCreator label: **{}** (author supplied)",
            markdown_inline(&creator.value)
        )
        .expect("writing to String cannot fail");
    }
    if let Some(agent) = &projection.presentation.proposal_agent_display_name {
        writeln!(
            output,
            "Proposal agent label: **{}** (author supplied)",
            markdown_inline(&agent.value)
        )
        .expect("writing to String cannot fail");
    }

    writeln!(output, "\n## Reading this history\n").expect("writing to String cannot fail");
    writeln!(
        output,
        "This view separates the original, the recorded current state, the AI-attributed proposal, and the Human decision. OIDs verify byte identity in the source repository; they do not prove authorship, truth, copyright, permission, or physical change.\n"
    )
    .expect("writing to String cannot fail");

    if projection.sessions.is_empty() {
        writeln!(
            output,
            "No complete creator session was available in the selected source snapshot.\n"
        )
        .expect("writing to String cannot fail");
    }
    for session in &projection.sessions {
        render_story_session(&mut output, session);
    }

    if !projection.incomplete_sessions.is_empty() {
        writeln!(output, "## Incomplete sessions\n").expect("writing to String cannot fail");
        writeln!(
            output,
            "These retained Ref shapes were not promoted into complete stories:\n"
        )
        .expect("writing to String cannot fail");
        for session in &projection.incomplete_sessions {
            writeln!(
                output,
                "- `{}` — proposal present: `{}`, decision present: `{}`",
                markdown_code(&session.session),
                session.proposal_present,
                session.decision_present
            )
            .expect("writing to String cannot fail");
        }
        output.push('\n');
    }

    writeln!(output, "## Disclosure and limits\n").expect("writing to String cannot fail");
    for limitation in &projection.limitations {
        writeln!(
            output,
            "- **{}** — {}",
            markdown_inline(&limitation.code),
            markdown_inline(&limitation.message)
        )
        .expect("writing to String cannot fail");
    }
    writeln!(
        output,
        "\nMachine-readable semantics are available in [`projection.json`](./projection.json). Machine readability does not grant training permission; this bundle declares `training_use_policy=prohibited`."
    )
    .expect("writing to String cannot fail");
    output
}

fn render_story_session(output: &mut String, session: &PublicSession) {
    writeln!(output, "## {}\n", markdown_inline(&session.title.value))
        .expect("writing to String cannot fail");
    writeln!(output, "Session: `{}`\n", markdown_code(&session.session))
        .expect("writing to String cannot fail");

    writeln!(output, "### Work history\n").expect("writing to String cannot fail");
    writeln!(
        output,
        "| Role | Public caption | Verified source OID | Rendering |"
    )
    .expect("writing to String cannot fail");
    writeln!(output, "|---|---|---|---|").expect("writing to String cannot fail");
    for artifact in &session.history {
        writeln!(
            output,
            "| {} | {} | `{}` | {} |",
            markdown_table(artifact.role.label()),
            markdown_table(&artifact.caption.value),
            markdown_code(&artifact.oid),
            markdown_table(&artifact.public_rendering.reason)
        )
        .expect("writing to String cannot fail");
    }
    output.push('\n');

    writeln!(output, "### Proposal and Human decision\n").expect("writing to String cannot fail");
    writeln!(
        output,
        "- Proposal attribution: {}",
        markdown_inline(&session.proposal.attribution_scope)
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Human disposition: **{}**",
        markdown_inline(&session.human_decision.disposition)
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Selected role: **{}**",
        markdown_inline(session.human_decision.selected_artifact.label())
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Proposal retained in history even when unselected: `{}`",
        session.proposal.retained_when_unselected
    )
    .expect("writing to String cannot fail");
    if let Some(note) = &session.human_decision.public_decision_note {
        writeln!(output, "\nPublic decision note (author supplied):\n")
            .expect("writing to String cannot fail");
        write_markdown_quote(output, &note.value);
        output.push('\n');
    } else {
        writeln!(
            output,
            "\nNo public decision note was supplied. The source rationale remains redacted because its stored visibility is private and its training-use policy is prohibited.\n"
        )
        .expect("writing to String cannot fail");
    }

    if let Some(comparison) = &session.comparison {
        writeln!(output, "### Evidence\n").expect("writing to String cannot fail");
        writeln!(
            output,
            "The current comparison reports **{}** with comparability **{}**. {}\n",
            markdown_inline(&comparison.outcome),
            markdown_inline(&comparison.comparability),
            markdown_inline(&comparison.interpretation_limit)
        )
        .expect("writing to String cannot fail");
    }

    writeln!(output, "### Technical provenance\n").expect("writing to String cannot fail");
    writeln!(
        output,
        "- Proposal Ref: `{}`\n- Decision Ref: `{}`\n- Base head: `{}`\n- Proposal head: `{}`\n- Decision head: `{}`\n- Projection fingerprint: `{}`\n- Objects verified by session fsck: `{}`\n",
        markdown_code(&session.provenance.proposal_ref),
        markdown_code(&session.provenance.decision_ref),
        markdown_code(&session.provenance.base_head),
        markdown_code(&session.provenance.proposal_head),
        markdown_code(&session.provenance.decision_head),
        markdown_code(&session.provenance.projection_source_fingerprint),
        session.provenance.fsck_objects_verified
    )
    .expect("writing to String cannot fail");
}

pub(crate) fn render_html(projection: &PublicProjection) -> String {
    let mut output = String::new();
    output.push_str("<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\n");
    output.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n");
    output.push_str("<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src 'self'; base-uri 'none'; form-action 'none'\">\n");
    writeln!(
        output,
        "<title>{}</title>",
        html(&projection.presentation.title.value)
    )
    .expect("writing to String cannot fail");
    output.push_str(STYLE);
    output.push_str("</head><body><main>\n");
    writeln!(
        output,
        "<header><p class=\"eyebrow\">SynapseGit provider-neutral publication view</p><h1>{}</h1><p class=\"lead\">{}</p><div class=\"badges\"><span>{}</span><span>network: 0</span></div>",
        html(&projection.presentation.title.value),
        html(&projection.presentation.summary.value),
        html(projection.publication.visibility.as_str())
    )
    .expect("writing to String cannot fail");
    if let Some(creator) = &projection.presentation.creator_display_name {
        write!(
            output,
            "<p class=\"byline\">Creator label: <strong>{}</strong> <span class=\"muted\">(author supplied)</span></p>",
            html(&creator.value)
        )
        .expect("writing to String cannot fail");
    }
    if let Some(agent) = &projection.presentation.proposal_agent_display_name {
        write!(
            output,
            "<p class=\"byline\">Proposal agent label: <strong>{}</strong> <span class=\"muted\">(author supplied)</span></p>",
            html(&agent.value)
        )
        .expect("writing to String cannot fail");
    }
    output.push_str("</header>\n");
    output.push_str("<section class=\"notice\"><h2>How to read this view</h2><p>Original, current state, AI-attributed proposal, and Human decision are distinct roles. OIDs verify source byte identity only; they do not prove authorship, truth, rights, permission, or physical change.</p></section>\n");

    if projection.sessions.is_empty() {
        output.push_str("<section><h2>No complete session</h2><p>The selected source snapshot did not contain a complete creator session.</p></section>\n");
    }
    for session in &projection.sessions {
        render_html_session(&mut output, session);
    }

    if !projection.incomplete_sessions.is_empty() {
        output.push_str("<section><h2>Incomplete sessions</h2><ul>");
        for session in &projection.incomplete_sessions {
            write!(
                output,
                "<li><code>{}</code> — proposal present: {}, decision present: {}</li>",
                html(&session.session),
                session.proposal_present,
                session.decision_present
            )
            .expect("writing to String cannot fail");
        }
        output.push_str("</ul></section>\n");
    }

    output.push_str("<section><h2>Disclosure and limits</h2><ul>");
    for limitation in &projection.limitations {
        write!(
            output,
            "<li><strong>{}</strong> — {}</li>",
            html(&limitation.code),
            html(&limitation.message)
        )
        .expect("writing to String cannot fail");
    }
    output.push_str("</ul><p>Machine-readable semantics: <a href=\"projection.json\">projection.json</a>. Machine readability does not grant training permission.</p></section>\n");
    output.push_str("</main></body></html>\n");
    output
}

fn render_html_session(output: &mut String, session: &PublicSession) {
    write!(
        output,
        "<article><p class=\"eyebrow\">session <code>{}</code></p><h2>{}</h2><div class=\"artifact-grid\">",
        html(&session.session),
        html(&session.title.value)
    )
    .expect("writing to String cannot fail");
    for artifact in &session.history {
        write!(
            output,
            "<section class=\"artifact\"><p class=\"role\">{}</p><h3>{}</h3><div class=\"placeholder\" aria-label=\"Asset bytes omitted\">Asset bytes omitted by policy</div><p>{}</p><code class=\"oid\">{}</code></section>",
            html(artifact.role.label()),
            html(&artifact.caption.value),
            html(&artifact.public_rendering.reason),
            html(&artifact.oid)
        )
        .expect("writing to String cannot fail");
    }
    output.push_str("</div>");
    let selected = session.human_decision.selected_artifact.label();
    write!(
        output,
        "<section class=\"decision\"><p class=\"eyebrow\">Human decision</p><h3>{}</h3><p>Selected role: <strong>{}</strong></p><p>Proposal retained in history even when unselected: <strong>{}</strong></p>",
        html(&session.human_decision.disposition),
        html(selected),
        session.proposal.retained_when_unselected
    )
    .expect("writing to String cannot fail");
    if let Some(note) = &session.human_decision.public_decision_note {
        write!(
            output,
            "<blockquote><p>{}</p><footer>Author-supplied public decision note</footer></blockquote>",
            html_with_breaks(&note.value)
        )
        .expect("writing to String cannot fail");
    } else {
        output.push_str("<p class=\"muted\">No public decision note supplied. The stored private rationale is redacted.</p>");
    }
    output.push_str("</section>");
    if let Some(comparison) = &session.comparison {
        write!(
            output,
            "<section><h3>Evidence</h3><p>Outcome: <strong>{}</strong>; comparability: <strong>{}</strong>.</p><p>{}</p></section>",
            html(&comparison.outcome),
            html(&comparison.comparability),
            html(&comparison.interpretation_limit)
        )
        .expect("writing to String cannot fail");
    }
    write!(
        output,
        "<details><summary>Technical provenance</summary><dl><dt>Proposal Ref</dt><dd><code>{}</code></dd><dt>Decision Ref</dt><dd><code>{}</code></dd><dt>Decision head</dt><dd><code>{}</code></dd><dt>Projection fingerprint</dt><dd><code>{}</code></dd></dl></details></article>",
        html(&session.provenance.proposal_ref),
        html(&session.provenance.decision_ref),
        html(&session.provenance.decision_head),
        html(&session.provenance.projection_source_fingerprint)
    )
    .expect("writing to String cannot fail");
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
    use super::{html, markdown_inline};

    #[test]
    fn escapes_active_markup() {
        assert_eq!(html("<script>&\"'"), "&lt;script&gt;&amp;&quot;&#39;");
        assert_eq!(markdown_inline("<script>|x"), "\\<script\\>\\|x");
    }
}
