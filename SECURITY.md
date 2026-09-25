# Politique de sécurité

## Versions prises en charge

| Version | Support |
|---|---|
| 0.1.x   | ✅ |

## Signaler une vulnérabilité

Si tu découvres une faille de sécurité, merci de **ne pas** ouvrir d'issue
publique. Signale-la de manière privée :

1. Ouvre un **rapport privé** via l'onglet *Security → Report a vulnerability*
   du dépôt, **ou**
2. Envoie un e-mail à l'adresse du mainteneur avec la description, les étapes
   de reproduction et l'impact estimé.

### Ce que tu peux attendre

- Accusé de réception sous 5 jours ouvrés.
- Confirmation de l'analyse et de la gravité.
- Correctif publié avant la divulgation publique (pratique de divulgation
  coordonnée).

## Bonnes pratiques de développement

- Aucune clé, jeton ou secret n'est stocké dans le dépôt.
- Les dépendances sont épinglées (`==`, `=`) et leurs licences vérifiées
  (notamment LAME, sous LGPL).
