import { useNavigate } from 'react-router-dom';
import { Workflow } from 'lucide-react';
import FlowsStatsView from '../components/flows/FlowsStatsView';

export default function FlowsStats() {
  const navigate = useNavigate();

  // Cliquer un flux/run dans la vue globale → ouvre le Studio sur l'app
  // concernée et préselectionne le flux (Studio sait déjà gérer ces query
  // params via FlowsTab — TODO si pas câblé : c'est tracé dans le plan).
  function handleSelectFlow(slug, flowName, runId) {
    if (!slug) return;
    const params = new URLSearchParams();
    params.set('app', slug);
    params.set('tab', 'flows');
    if (flowName) params.set('flow', flowName);
    if (runId) params.set('run', runId);
    navigate(`/studio?${params.toString()}`);
  }

  return (
    <div className="h-full flex flex-col bg-gray-900">
      <div className="px-5 py-3 border-b border-gray-700 bg-gray-800/60 flex items-center gap-2 shrink-0">
        <Workflow className="w-4 h-4 text-blue-400" />
        <h2 className="text-sm font-semibold text-white">Flow Stats</h2>
        <span className="text-[11px] text-gray-500">Toutes applications confondues</span>
      </div>
      <div className="flex-1 overflow-hidden">
        <FlowsStatsView scope="global" onSelectFlow={handleSelectFlow} />
      </div>
    </div>
  );
}
