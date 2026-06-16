// Badge mono coloré du status d'un fichier git (A/M/D/R/C/T/U). Partagé entre le
// panneau Git (liste des modifs) et la visionneuse de diff du working tree.
const STATUS_STYLE = {
  A: 'text-green-400 bg-green-900/30',
  M: 'text-yellow-400 bg-yellow-900/30',
  D: 'text-red-400 bg-red-900/30',
  R: 'text-blue-400 bg-blue-900/30',
  C: 'text-cyan-400 bg-cyan-900/30',
  T: 'text-purple-400 bg-purple-900/30',
  U: 'text-orange-400 bg-orange-900/30',
};

export default function FileStatusBadge({ status }) {
  const s = (status || 'X').toUpperCase().charAt(0);
  return (
    <span className={`w-5 text-center text-[11px] font-mono font-bold shrink-0 rounded-sm ${STATUS_STYLE[s] || 'text-gray-400 bg-gray-700/40'}`}>
      {s}
    </span>
  );
}
