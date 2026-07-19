use crate::{
    GenericArtifactOutcomeState, GenericArtifactPublicProjectionV1, GenericArtifactVisibility,
};
use std::fmt::Write;

pub(crate) fn render_generic_artifact_views(
    projection: &GenericArtifactPublicProjectionV1,
) -> (Vec<u8>, Vec<u8>) {
    (
        render_generic_artifact_markdown(projection).into_bytes(),
        render_generic_artifact_html(projection).into_bytes(),
    )
}

fn render_generic_artifact_markdown(projection: &GenericArtifactPublicProjectionV1) -> String {
    let target = projection.public_target.target();
    let outcome = &projection.outcome;
    let mut output = String::new();
    writeln!(output, "# {}", markdown(target.label())).unwrap();
    writeln!(output).unwrap();
    writeln!(
        output,
        "Provider-neutral generic artifact publication for reviewed LP Studio target `{}`.",
        markdown_code(target.target_id())
    )
    .unwrap();
    writeln!(output).unwrap();
    writeln!(output, "## Public target").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "| Field | Value |").unwrap();
    writeln!(output, "| --- | --- |").unwrap();
    writeln!(output, "| Kind | `{}` |", target.kind().as_str()).unwrap();
    writeln!(
        output,
        "| Capture source | `{}` |",
        target.capture_source().as_str()
    )
    .unwrap();
    writeln!(
        output,
        "| LP Studio | `{}` |",
        markdown_code(projection.contracts.lp_studio.product_version())
    )
    .unwrap();
    writeln!(
        output,
        "| API / schema / Target schema | `{}` / `{}` / `{}` |",
        projection.contracts.lp_studio.api_version(),
        projection.contracts.lp_studio.api_schema_version(),
        projection.contracts.lp_studio.target_schema_version()
    )
    .unwrap();
    writeln!(output).unwrap();
    writeln!(output, "## Review outcome").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "- State: `{}`", outcome.state.as_str()).unwrap();
    writeln!(
        output,
        "- Attribution: `caller_supplied_ai_attributed` (`execution_verified=false`)"
    )
    .unwrap();
    if let Some(disposition) = outcome.human_disposition {
        writeln!(output, "- Human disposition: `{}`", disposition.as_str()).unwrap();
    }
    if let Some(snapshot) = outcome.selected_snapshot {
        writeln!(output, "- Selected snapshot: `{}`", snapshot.as_str()).unwrap();
    }
    if let Some(reason) = outcome.status_reason {
        writeln!(output, "- Status reason: `{}`", reason.as_str()).unwrap();
    }
    if let Some(site) = &outcome.accepted_site {
        writeln!(
            output,
            "- Accepted site manifest SHA-256: `{}`",
            site.manifest_sha256
        )
        .unwrap();
        writeln!(output, "- Verified file count: `{}`", site.file_count).unwrap();
        writeln!(output, "- Verified total bytes: `{}`", site.total_bytes).unwrap();
        writeln!(output, "- Public Core OID included: `false`").unwrap();
    }
    writeln!(output).unwrap();
    writeln!(output, "### Verification scope").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "{}", markdown(&outcome.verification_scope)).unwrap();
    writeln!(output).unwrap();
    writeln!(output, "## Limitations").unwrap();
    writeln!(output).unwrap();
    for limitation in &projection.limitations {
        writeln!(
            output,
            "- **{}:** {}",
            markdown(&limitation.code),
            markdown(&limitation.message)
        )
        .unwrap();
    }
    writeln!(output).unwrap();
    writeln!(
        output,
        "Machine-readable semantics are in [`projection.json`](./projection.json). This local bundle performed zero network and zero Git operations."
    )
    .unwrap();
    output
}

fn render_generic_artifact_html(projection: &GenericArtifactPublicProjectionV1) -> String {
    let target = projection.public_target.target();
    let outcome = &projection.outcome;
    let state_class = match outcome.state {
        GenericArtifactOutcomeState::Complete => "complete",
        GenericArtifactOutcomeState::Pending => "pending",
        GenericArtifactOutcomeState::Incomplete => "incomplete",
    };
    let visibility = match projection.publication.visibility {
        GenericArtifactVisibility::PrivateReview => "Private review",
        GenericArtifactVisibility::Public => "Public",
    };
    let mut details = String::new();
    if let Some(disposition) = outcome.human_disposition {
        write!(
            details,
            "<dt>Human disposition</dt><dd><code>{}</code></dd>",
            disposition.as_str()
        )
        .unwrap();
    }
    if let Some(snapshot) = outcome.selected_snapshot {
        write!(
            details,
            "<dt>Selected snapshot</dt><dd><code>{}</code></dd>",
            snapshot.as_str()
        )
        .unwrap();
    }
    if let Some(reason) = outcome.status_reason {
        write!(
            details,
            "<dt>Status reason</dt><dd><code>{}</code></dd>",
            reason.as_str()
        )
        .unwrap();
    }
    if let Some(site) = &outcome.accepted_site {
        write!(
            details,
            "<dt>Accepted site manifest SHA-256</dt><dd><code>{}</code></dd><dt>Verified files</dt><dd>{}</dd><dt>Verified bytes</dt><dd>{}</dd><dt>Public Core OID</dt><dd>Not included</dd>",
            site.manifest_sha256, site.file_count, site.total_bytes
        )
        .unwrap();
    }
    let limitations = projection
        .limitations
        .iter()
        .map(|limitation| {
            format!(
                "<li><strong>{}</strong>: {}</li>",
                html(&limitation.code),
                html(&limitation.message)
            )
        })
        .collect::<String>();
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title>{STYLE}</head><body><main><header><p class=\"eyebrow\">Generic artifact · {visibility}</p><h1>{}</h1><p>Reviewed LP Studio public target <code>{}</code>.</p><div class=\"badge {state_class}\">{}</div></header><section><h2>Public target</h2><dl><dt>Kind</dt><dd><code>{}</code></dd><dt>Capture source</dt><dd><code>{}</code></dd><dt>LP Studio version</dt><dd><code>{}</code></dd><dt>API / schema / Target schema</dt><dd><code>{}</code> / <code>{}</code> / <code>{}</code></dd></dl></section><section><h2>Review outcome</h2><dl><dt>State</dt><dd><code>{}</code></dd><dt>Attribution</dt><dd><code>caller_supplied_ai_attributed</code>; execution was not verified.</dd>{details}</dl><h3>Verification scope</h3><p>{}</p></section><section><h2>Limitations</h2><ul>{limitations}</ul></section><footer>Machine-readable semantics are in <code>projection.json</code>. This local bundle performed zero network and zero Git operations.</footer></main></body></html>\n",
        html(target.label()),
        html(target.label()),
        html(target.target_id()),
        outcome.state.as_str(),
        target.kind().as_str(),
        target.capture_source().as_str(),
        html(projection.contracts.lp_studio.product_version()),
        projection.contracts.lp_studio.api_version(),
        projection.contracts.lp_studio.api_schema_version(),
        projection.contracts.lp_studio.target_schema_version(),
        outcome.state.as_str(),
        html(&outcome.verification_scope),
    )
}

fn markdown(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(
            character,
            '\\' | '`'
                | '*'
                | '_'
                | '{'
                | '}'
                | '['
                | ']'
                | '<'
                | '>'
                | '('
                | ')'
                | '#'
                | '+'
                | '-'
                | '.'
                | '!'
                | '|'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn markdown_code(value: &str) -> String {
    value.replace('`', "\\`")
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
            ':' => escaped.push_str("&#58;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

const STYLE: &str = r#"<style>
:root{color-scheme:light;--ink:#172019;--muted:#5d665f;--paper:#f5f2e9;--panel:#fffdf7;--line:#d9d3c5;--ok:#276749;--wait:#8a5b00;--bad:#8f3030}*{box-sizing:border-box}body{margin:0;background:var(--paper);color:var(--ink);font:16px/1.6 ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}main{width:min(920px,calc(100% - 2rem));margin:auto;padding:3rem 0 5rem}header,section{background:var(--panel);border:1px solid var(--line);border-radius:16px;padding:clamp(1.2rem,3vw,2.2rem);margin-bottom:1.25rem}h1{font-size:clamp(2rem,6vw,4rem);line-height:1.05;overflow-wrap:anywhere}.eyebrow{text-transform:uppercase;letter-spacing:.1em;font-size:.78rem;font-weight:750;color:var(--muted)}.badge{display:inline-block;border:1px solid;border-radius:999px;padding:.25rem .75rem;font-weight:700}.complete{color:var(--ok)}.pending{color:var(--wait)}.incomplete{color:var(--bad)}dt{font-weight:750;margin-top:.75rem}dd{margin-left:0;overflow-wrap:anywhere}code{overflow-wrap:anywhere}footer{color:var(--muted);padding:1rem}
</style>"#;

#[cfg(test)]
mod tests {
    use super::{html, markdown};

    #[test]
    fn generic_renderer_escapes_active_markup() {
        assert_eq!(html("<script>&\"':"), "&lt;script&gt;&amp;&quot;&#39;&#58;");
        assert_eq!(
            markdown("[x](javascript:y)|<z>"),
            "\\[x\\]\\(javascript:y\\)\\|\\<z\\>"
        );
    }
}
