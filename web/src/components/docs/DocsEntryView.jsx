import MarkdownView from './MarkdownView';
import MermaidDiagram from './MermaidDiagram';
import EntryTypeBadge from './EntryTypeBadge';
import { Link as LinkIcon, FileCode, Calendar } from 'lucide-react';

function formatDate(s) {
  if (!s) return null;
  try {
    const d = new Date(s);
    return d.toLocaleString();
  } catch {
    return s;
  }
}

export default function DocsEntryView({ entry, onPickLink }) {
  if (!entry) return null;
  const { frontmatter: fm, body, type, name, diagram } = entry;
  const updated = formatDate(fm?.updated_at);

  return (
    <article className="space-y-4">
      {/* Header */}
      <header>
        <div className="flex items-center gap-2 mb-2 flex-wrap">
          <EntryTypeBadge
            type={type}
            scope={fm?.scope}
            parentScreen={fm?.parent_screen}
          />
          <span className="text-xs text-gray-500 font-mono">{name}</span>
          {updated && (
            <span className="text-xs text-gray-500 flex items-center gap-1 ml-auto">
              <Calendar className="w-3 h-3" />
              maj {updated}
            </span>
          )}
        </div>
        {fm?.title && (
          <h2 className="text-2xl font-bold text-white">{fm.title}</h2>
        )}
        {fm?.summary && (
          <p className="text-gray-400 mt-1">{fm.summary}</p>
        )}
      </header>

      {/* Code refs */}
      {Array.isArray(fm?.code_refs) && fm.code_refs.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {fm.code_refs.map((ref) => (
            <span
              key={ref}
              className="inline-flex items-center gap-1 px-2 py-0.5 text-xs rounded
                         bg-gray-800/60 border border-gray-700 text-gray-300 font-mono"
              title="Référence code"
            >
              <FileCode className="w-3 h-3 text-gray-500" />
              {ref}
            </span>
          ))}
        </div>
      )}

      {/* Links to other entries */}
      {Array.isArray(fm?.links) && fm.links.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {fm.links.map((link) => {
            // link format: "type:name"
            const [t, ...rest] = link.split(':');
            const n = rest.join(':');
            return (
              <button
                key={link}
                onClick={() => onPickLink?.({ type: t, name: n })}
                className="inline-flex items-center gap-1 px-2 py-0.5 text-xs rounded
                           bg-blue-500/10 border border-blue-500/30 text-blue-300
                           hover:bg-blue-500/20"
              >
                <LinkIcon className="w-3 h-3" />
                {link}
              </button>
            );
          })}
        </div>
      )}

      {/* Body markdown */}
      <div className="border-t border-gray-800 pt-4">
        <MarkdownView>{body}</MarkdownView>
      </div>

      {/* Diagram */}
      {diagram && (
        <div>
          <div className="text-xs uppercase tracking-wider text-gray-500 mb-2">
            Diagramme
          </div>
          <MermaidDiagram code={diagram} />
        </div>
      )}
    </article>
  );
}
