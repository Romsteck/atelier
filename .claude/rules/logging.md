# Logging — Atelier

Tout nouveau code DOIT logger structuré via `tracing`.

## Patterns obligatoires

```rust
use tracing::{info, warn, error, instrument};

#[instrument(skip(state))]
async fn my_handler(State(state): State<ApiState>, Path(slug): Path<String>) -> impl IntoResponse {
    info!(slug = %slug, "starting operation X");
    // ...
}
```

- `#[instrument]` pour spans automatiques (active le champ `function` dans LogSource)
- `skip(state)` ou `skip(...)` pour les paramètres volumineux
- Champs structurés `info!(field = value, "msg")`, **pas** d'interpolation

## Niveaux

| Niveau | Usage |
|---|---|
| `error!` | Échec qui empêche l'opération, perte de données |
| `warn!` | Inattendu mais géré, fallback, retry |
| `info!` | Opération significative : requête HTTP, IPC call, mutation |
| `debug!` | Diagnostic : valeurs intermédiaires |
| `trace!` | Très verbeux : contenu requêtes/réponses |

## Ne PAS logger

- Tokens, secrets, mots de passe — JAMAIS
- Bodies HTTP volumineux — `trace!` si vraiment nécessaire
- Boucles serrées (poll < 5s) — sauf changement de valeur
