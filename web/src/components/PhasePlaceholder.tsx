import { Hammer, ExternalLink } from "lucide-react";

interface Props {
  phase: number;
  feature: string;
  description: string;
}

export default function PhasePlaceholder({
  phase,
  feature,
  description,
}: Props) {
  return (
    <div className="max-w-2xl">
      <div className="flex items-center gap-2 text-amber-400 mb-2">
        <Hammer className="w-5 h-5" />
        <span className="text-sm font-medium uppercase tracking-wider">
          Phase {phase} — à venir
        </span>
      </div>
      <h2 className="text-xl font-semibold mb-3">{feature}</h2>
      <p className="text-gray-400 mb-6 leading-relaxed">{description}</p>
      <div className="bg-gray-900 border border-gray-800 rounded-md p-4 text-sm">
        <div className="text-gray-500 text-xs uppercase tracking-wider mb-2">
          Plan de migration
        </div>
        <p className="text-gray-300 mb-3">
          Migration depuis homeroute en{" "}
          <strong>9 phases progressives</strong>, parallèle, sans downtime.
          Chaque feature est portée d'abord en read-only puis en read-write.
        </p>
        <a
          href="https://proxy.mynetwk.biz/api/docs"
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex items-center gap-1 text-amber-400 hover:underline"
        >
          Référence: homeroute actuelle
          <ExternalLink className="w-3.5 h-3.5" />
        </a>
      </div>
    </div>
  );
}
