# HTML Formatting Guide

Azure DevOps work item descriptions and comments are HTML. The `amplihack-rs` bundle does not ship the legacy Markdown-to-HTML Python formatter, so convert text before sending it to Azure DevOps.

## Simple Inline HTML

```bash
az boards work-item create \
  --type "User Story" \
  --title "My Story" \
  --description "<h1>Story</h1><p>This is <strong>bold</strong> text.</p><ul><li>Criterion 1</li><li>Criterion 2</li></ul>"
```

## From Markdown

Use an available project tool such as `pandoc`, a documentation generator, or an editor export to produce HTML, then pass that HTML to Azure CLI:

```bash
pandoc story.md -t html -o story.html
az boards work-item create \
  --type "User Story" \
  --title "My Story" \
  --description "$(cat story.html)"
```

If no converter is available, write minimal HTML directly. Azure DevOps supports standard tags such as `<p>`, `<strong>`, `<em>`, `<ul>`, `<ol>`, `<li>`, `<h1>`, `<h2>`, `<pre>`, and `<code>`.
