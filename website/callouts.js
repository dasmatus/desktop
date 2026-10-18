// Callouts. The pages mark them as GitHub does,
//
//   > [!WARNING]
//   > This erases the disk.
//
// which GitHub and the wiki draw as a coloured box and Docusaurus, reading
// plain Markdown, would draw as a quote with "[!WARNING]" in it. This turns
// each such quote into the directive Docusaurus's own `:::warning` makes, so
// the site and the offline copy draw its admonition; `:::` itself would show
// as text on GitHub and in the wiki. It runs before Docusaurus's admonitions
// plugin, which takes it from there.

const KINDS = new Set(['note', 'tip', 'important', 'warning', 'caution']);
const MARKER = /^\[!([A-Za-z]+)\][ \t]*(?:\n|$)/;

function callouts() {
  return (root) => {
    walk(root);
  };
}

function walk(node) {
  if (!node.children) return;
  for (const child of node.children) {
    if (child.type === 'blockquote') convert(child);
    walk(child);
  }
}

function convert(quote) {
  const first = quote.children[0];
  const text = first && first.type === 'paragraph' ? first.children[0] : undefined;
  if (!text || text.type !== 'text') return;
  const match = text.value.match(MARKER);
  if (!match || !KINDS.has(match[1].toLowerCase())) return;

  // The marker is a line of its own; what follows it on the next lines is
  // the callout's first paragraph, and a marker alone in its paragraph
  // leaves nothing of it.
  text.value = text.value.slice(match[0].length);
  if (text.value === '') first.children.shift();
  if (first.children.length === 0) quote.children.shift();

  quote.type = 'containerDirective';
  quote.name = match[1].toLowerCase();
  quote.attributes = {};
}

// A page's description, for search results and link previews, is its first
// line of text, which on a page that opens with a callout is the marker.
// This is the same excerpt Docusaurus takes, from the text without markers.
function description(content) {
  if (!/^>[ \t]*\[![A-Za-z]+\][ \t]*$/m.test(content)) return undefined;
  const { createExcerpt } = require('@docusaurus/utils');
  return createExcerpt(content.replace(/^>[ \t]*\[![A-Za-z]+\][ \t]*\n/gm, ''));
}

module.exports = callouts;
module.exports.description = description;
