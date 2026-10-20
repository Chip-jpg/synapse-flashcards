import type { Block, Inline } from "./markdown";

/**
 * A note's text shown formatted as Markdown, from the `blocks` `parseMarkdown`
 * found in it. Everything is drawn as React elements, so no part of the note
 * is ever read as HTML; links and pictures are described, never opened or
 * loaded.
 */
export function MarkdownView({ blocks }: { blocks: Block[] }) {
  return <div className="note-body markdown">{renderBlocks(blocks)}</div>;
}

function renderBlocks(blocks: Block[]) {
  return blocks.map((block, index) => <BlockView key={index} block={block} />);
}

function BlockView({ block }: { block: Block }) {
  switch (block.type) {
    case "heading": {
      // The note's title is the page's h2, so its headings start below it.
      const Tag = `h${Math.min(block.level + 2, 6)}` as "h3" | "h4" | "h5" | "h6";
      return <Tag className={`markdown-h${block.level}`}>{renderInline(block.content)}</Tag>;
    }
    case "paragraph":
      return <p>{renderInline(block.content)}</p>;
    case "code":
      return (
        <pre className="markdown-code">
          <code>{block.text}</code>
        </pre>
      );
    case "quote":
      return <blockquote>{renderBlocks(block.blocks)}</blockquote>;
    case "list": {
      const items = block.items.map((item, index) => (
        <li key={index} className={item.task === null ? undefined : "markdown-task"}>
          {item.task !== null && (
            <span
              className="markdown-check"
              role="img"
              aria-label={item.task ? "Done" : "Not done"}
            >
              {item.task ? "☑" : "☐"}
            </span>
          )}
          {renderBlocks(item.blocks)}
        </li>
      ));
      return block.ordered ? (
        <ol start={block.start === 1 ? undefined : block.start}>{items}</ol>
      ) : (
        <ul>{items}</ul>
      );
    }
    case "rule":
      return <hr />;
    case "table":
      return (
        // Focusable so keyboard users can scroll a wide table.
        <div className="markdown-table" tabIndex={0}>
          <table>
            <thead>
              <tr>
                {block.head.map((cell, column) => (
                  <th key={column} style={{ textAlign: block.align[column] ?? undefined }}>
                    {renderInline(cell)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {block.rows.map((row, index) => (
                <tr key={index}>
                  {row.map((cell, column) => (
                    <td key={column} style={{ textAlign: block.align[column] ?? undefined }}>
                      {renderInline(cell)}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
  }
}

function renderInline(nodes: Inline[]) {
  return nodes.map((node, index) => <InlineView key={index} node={node} />);
}

function InlineView({ node }: { node: Inline }) {
  switch (node.type) {
    case "text":
      return node.text;
    case "code":
      return <code>{node.text}</code>;
    case "em":
      return <em>{renderInline(node.children)}</em>;
    case "strong":
      return <strong>{renderInline(node.children)}</strong>;
    case "del":
      return <del>{renderInline(node.children)}</del>;
    case "link": {
      const shown =
        node.children.length === 1 && node.children[0].type === "text"
          ? node.children[0].text
          : null;
      return (
        <span className="markdown-link">
          <span className="markdown-link-text">{renderInline(node.children)}</span>
          {shown !== node.url && node.url !== "" && (
            <span className="markdown-url">{` (${node.url})`}</span>
          )}
        </span>
      );
    }
    case "image":
      return (
        <span className="markdown-image">
          {node.alt === "" ? "Picture" : `Picture: ${node.alt}`}
          {node.url !== "" && <span className="markdown-url">{` (${node.url})`}</span>}
        </span>
      );
  }
}
