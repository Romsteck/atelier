import MarkdownView from './MarkdownView';
import MermaidDiagram from './MermaidDiagram';
import { Layout, Layers, Boxes, GitBranch } from 'lucide-react';

function StatCard({ icon: Icon, label, value, sub }) {
  return (
    <div className="px-4 py-3 bg-gray-800/40 border border-gray-700 rounded-lg flex items-center gap-3">
      <Icon className="w-5 h-5 text-blue-400 flex-shrink-0" />
      <div>
        <div className="text-2xl font-bold text-white leading-none">{value}</div>
        <div className="text-xs text-gray-400 mt-0.5">{label}</div>
        {sub && <div className="text-[10px] text-gray-500">{sub}</div>}
      </div>
    </div>
  );
}

export default function DocsOverview({ overview, diagram }) {
  if (!overview) return null;
  const { meta, overview: ovEntry, stats } = overview;

  return (
    <div className="space-y-6">
      {/* Meta header */}
      <div>
        <h1 className="text-2xl font-bold text-white">{meta.name}</h1>
        {meta.description && (
          <p className="text-gray-400 mt-1">{meta.description}</p>
        )}
        {meta.stack && (
          <span className="inline-block mt-2 px-2 py-0.5 text-xs rounded
                           bg-blue-500/20 text-blue-300 border border-blue-500/30">
            {meta.stack}
          </span>
        )}
      </div>

      {/* Stats */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
        <StatCard icon={Layout} label="Écrans" value={stats?.screens ?? 0} />
        <StatCard icon={Layers} label="Features" value={stats?.features ?? 0} />
        <StatCard icon={Boxes} label="Composants" value={stats?.components ?? 0} />
        <StatCard
          icon={GitBranch}
          label="Diagrammes"
          value={stats?.with_diagram ?? 0}
          sub="entrées avec mermaid"
        />
      </div>

      {/* Overview body or empty state */}
      {ovEntry ? (
        <div className="space-y-4">
          {ovEntry.frontmatter?.title && (
            <h2 className="text-xl font-semibold text-white">
              {ovEntry.frontmatter.title}
            </h2>
          )}
          <MarkdownView>{ovEntry.body}</MarkdownView>
          {diagram && (
            <div>
              <div className="text-xs uppercase tracking-wider text-gray-500 mb-2">
                Diagramme global
              </div>
              <MermaidDiagram code={diagram} />
            </div>
          )}
        </div>
      ) : (
        <div className="p-6 bg-gray-800/30 border border-dashed border-gray-700 rounded-lg text-center text-gray-400">
          <p className="font-medium text-gray-300 mb-1">
            Aucune vue d'ensemble pour cette app.
          </p>
          <p className="text-sm">
            L'agent peut la créer via <code className="px-1 py-0.5 bg-gray-900 rounded text-xs text-blue-300">docs_update(type=overview, name=overview, …)</code>.
          </p>
        </div>
      )}
    </div>
  );
}
