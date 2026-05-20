# Prompt de migration — apps NextJS vers `hr-flowd` callback

> Copie tout ce qui est sous le séparateur dans la conversation de l'agent de l'app NextJS cible.

---

Migration vers hr-flow (mode callback / daemon partagé, stack NextJS).

**Contexte plateforme** : `hr-flow` tourne en daemon partagé `hr-flowd` (Medion `127.0.0.1:4002`, loopback only). Tes flux TOML sont chargés au boot du daemon et leurs actions / connecteurs custom sont exécutés en HTTP côté ton app via `POST /_flow/action/{name}` et `POST /_flow/connector/{name}/{op}`. **Pas de Rust dans ta toolchain** — tout reste TS / Node.

C'est la première fois que cette app touche aux flux. Tu suis ce prompt et tu as accès au skill `flow-build` (lazy-loaded) si tu veux le détail TOML.

**Étapes — fais-les dans l'ordre, vérifie après chaque.**

### 1. Audit (rends-moi la liste avant de coder)

Repère dans `app/api/` toutes les routes Next qui chaînent **≥ 2 étapes** : fetch DB + transformation + write, appel externe + cache, boucle/condition portant sur des données métier. Renvoie-moi la liste classée par priorité (haut = simple à migrer / fort impact métier), une phrase métier par route.

### 2. Intégration plateforme (une fois la liste validée)

- `package.json` : ajoute la dep
  ```json
  "@homeroute/flow-action": "file:/nvme/atelier/web/packages/flow-action"
  ```
  (path-dep le temps de la transition ; un publish npm interne arrivera à terme)

- `npm install` (ou `pnpm install`)

- Crée le **catchall** `app/api/_flow/[type]/[name]/route.ts` :
  ```ts
  import { handleFlowCallback } from '@homeroute/flow-action';
  // import { greet } from '@/lib/flow-actions/greet';

  export const runtime = 'nodejs'; // requis : Edge runtime ne fournit pas crypto.timingSafeEqual

  export const POST = handleFlowCallback({
    actions: {
      // greet,
    },
    connectors: {
      // openrouter: {
      //   chat: async (input, params, ctx) => { /* ... */ },
      // },
    },
  });
  ```

- Pour les connecteurs avec routing par op (`/_flow/connector/{name}/{op}`), Next a besoin d'un **second** catchall plus profond. Si ton app utilise des connecteurs custom :
  ```
  app/api/_flow/[type]/[name]/[op]/route.ts
  ```
  qui ré-exporte le même handler — `handleFlowCallback` interpête l'URL.
  Si ton app n'utilise QUE des actions (pas de connecteur custom), tu peux laisser le second catchall absent.

- `.env.local` (ou `.env` selon la convention de l'app) :
  ```
  HR_FLOW_TOKEN=<token>
  ```
  Le token (32 bytes hex) est aussi à ajouter à `apps.json` côté Medion sur l'entrée de l'app, sous `flow_callback_url` (`http://127.0.0.1:<port>`) et `flow_callback_token`. Vérifie avec `mcp__atelier__app.get_app(slug=...)` ou `mcp__studio__app.regenerate_flow_token(slug=...)`.

- Crée le dossier `flows/` à la racine de l'app (au même niveau que `app/`, `lib/`, `package.json`).

- **Pas de `build_artefact` à modifier** (le `next build` standard inclut tout le sous-répertoire `app/`).

- `npm run build` puis `make deploy-app SLUG=<slug>`.

### 3. Migration par lot

Pour chaque route Next candidate :

1. Crée `flows/<nom>.toml` (format plat parent / parent_branch ; cf. skill `flow-build`).

2. Si la route a besoin de logique custom non couverte par les primitives, écris une **action TS pure** dans `lib/flow-actions/<nom>.ts` :
   ```ts
   import type { FlowAction } from '@homeroute/flow-action';

   export const compute_score: FlowAction = async (input, params, ctx) => {
     // input et params sont les données envoyées par le daemon
     return { score: 0.42 };
   };
   ```
   Importe-la dans `app/api/_flow/[type]/[name]/route.ts` et ajoute-la au map `actions`.

3. Le handler Next d'origine devient un wrapper mince :
   ```ts
   export async function POST(req: Request) {
     const body = await req.json();
     const r = await fetch('http://127.0.0.1:4100/api/apps/<slug>/flows/<nom>/run', {
       method: 'POST',
       headers: { 'content-type': 'application/json' },
       body: JSON.stringify({ input: body }),
     });
     return r;
   }
   ```
   (Tu peux aussi garder le handler intact pendant la phase de test et appeler le flux depuis un endpoint distinct.)

4. Build, déploie, teste via `mcp__studio__flow.run(slug=<slug>, name=<nom>, input=...)`.

5. Itère sur le lot suivant.

### 4. Escalade plateforme — règle stricte

Si tu rencontres un **bug** ou une **limitation** dans `hr-flow` / `@homeroute/flow-action` / `hr-flowd` (engine, primitive, expression, callback contract, persistence) :

1. **STOP. NE CONTOURNE PAS.** Pas d'action TS qui contourne une primitive manquante. Pas de hack qui maquille le bug.
2. **Rapport structuré** dans la conversation, en français :
   - **Sévérité** : `P0` / `P1` / `P2`
   - **Contexte** : quel flux, quel step, quel input réel
   - **Repro minimale** : extrait TOML + ce que tu attends + ce qui se passe
   - **Hypothèse** sur la cause si tu en as une
3. **Attends le correctif**. Plateforme dans `/nvme/atelier/crates/hr-flow*` + `/nvme/atelier/web/packages/flow-action/`.
4. Pas de TODO bidouille : passe à un autre lot ou autre tâche.

### Démarre par l'étape 1 : l'audit.
