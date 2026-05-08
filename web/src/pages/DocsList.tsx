import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { AppCard, listApps } from "../api";

export default function DocsList() {
  const [apps, setApps] = useState<AppCard[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    listApps()
      .then(setApps)
      .catch((e) => setError(String(e)));
  }, []);

  if (error) return <p className="error">{error}</p>;
  if (!apps) return <p>Chargement…</p>;

  return (
    <div>
      <h1>Apps documentées</h1>
      <p className="muted">
        {apps.length} apps — données synchronisées depuis Medion (toutes les
        5 min).
      </p>
      <ul className="cards">
        {apps.map((a) => (
          <li key={a.app_id} className="card">
            <Link to={`/docs/${a.app_id}`} className="card-link">
              <div className="card-head">
                <span className="logo">{a.logo || "📄"}</span>
                <span className="name">{a.name || a.app_id}</span>
              </div>
              {a.description && (
                <p className="desc">{a.description}</p>
              )}
              {a.stack && <p className="stack">{a.stack}</p>}
              <div className="stats">
                <span>{a.stats.screens} screens</span>
                <span>{a.stats.features} features</span>
                <span>{a.stats.components} components</span>
                <span>{a.stats.with_diagram} diagrams</span>
              </div>
            </Link>
          </li>
        ))}
      </ul>
    </div>
  );
}
