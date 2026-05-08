# Plan suivant — `hr-flowd` daemon multi-stack

Une fois la migration depuis homeroute terminée (cutover Phase 9 du plan en cours), enchaîner **directement** sur le plan suivant :

📄 [/home/romain/.claude/plans/peaceful-spinning-mountain.md](/home/romain/.claude/plans/peaceful-spinning-mountain.md)

**Objectif** : transformer `hr-flow` (aujourd'hui lib Rust embeddable, donc inutilisable côté NextJS) en daemon partagé `hr-flowd` accessible via callbacks HTTP par toutes les apps quelle que soit la stack (Rust ou NextJS).

**Pourquoi c'est important maintenant** : pendant la migration de `hr-flow` vers Atelier (Phase 5/6 du plan en cours), il faut garder en tête que la cible n'est plus la lib embedded mais un daemon. Concrètement : ne pas coupler fortement `hr-flow` à `ApiState` Atelier ou au runtime des apps Atelier — sinon l'extraction du daemon devient plus douloureuse.

Le plan détaille 7 phases : daemon → RemoteEngine → callback NextJS → bascule Wallet → roll-out apps Rust → roll-out apps NextJS → scaffold automation.
