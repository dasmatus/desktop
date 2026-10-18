// GitHub's callout quotes become the directives Docusaurus draws as
// admonitions (callouts.js), and every other quote stays a quote.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const callouts = require("../callouts.js");

const text = (value) => ({ type: "text", value });
const paragraph = (...children) => ({ type: "paragraph", children });
const quote = (...children) => ({ type: "blockquote", children });
const run = (...children) => {
  const root = { type: "root", children };
  callouts()(root);
  return root.children;
};

test("a marker on its own line makes the quote a directive of that kind", () => {
  const [node] = run(quote(paragraph(text("[!WARNING]\nThis erases the disk."))));
  assert.equal(node.type, "containerDirective");
  assert.equal(node.name, "warning");
  assert.deepEqual(node.children, [paragraph(text("This erases the disk."))]);
});

test("a marker alone in its paragraph leaves no empty paragraph behind", () => {
  const [node] = run(quote(paragraph(text("[!NOTE]")), paragraph(text("Body."))));
  assert.equal(node.name, "note");
  assert.deepEqual(node.children, [paragraph(text("Body."))]);
});

test("a marker followed by code keeps the code", () => {
  const code = { type: "inlineCode", value: "pm" };
  const [node] = run(quote(paragraph(text("[!TIP]\n"), code, text(" does it."))));
  assert.equal(node.name, "tip");
  assert.deepEqual(node.children, [paragraph(code, text(" does it."))]);
});

test("each of GitHub's five kinds is recognised, in any case", () => {
  for (const kind of ["NOTE", "Tip", "important", "WARNING", "CAUTION"]) {
    const [node] = run(quote(paragraph(text(`[!${kind}]\nx`))));
    assert.equal(node.name, kind.toLowerCase());
  }
});

test("an ordinary quote, an unknown kind or a marker mid-line stays a quote", () => {
  for (const value of ["Just a quote.", "[!DANGER]\nx", "[!NOTE] inline", "See [!NOTE]\nx"]) {
    const [node] = run(quote(paragraph(text(value))));
    assert.equal(node.type, "blockquote", value);
  }
});

test("a page opening with a callout is described by its text, not its marker", () => {
  const page = "# Page\n\n> [!IMPORTANT]\n> Clone with submodules.\n\nMore.\n";
  assert.equal(callouts.description(page), "Clone with submodules.");
  assert.equal(callouts.description("# Page\n\nPlain.\n"), undefined);
});
