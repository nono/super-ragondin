# Proposition alternative à la OT

Erwan m'a rapidement parlé de cette proposition qu'il avait en tête. J'ai
essayé de mettre dans ce document ce que j'en avais compris, puis d'expliquer
pourquoi cette proposition ne m'inspire guère confiance.

## Fonctionnement

On utilise 4 bases de données, 2 pour la partie locale et 2 pour la partie
distante, avec à chaque fois une base qui représente le système de fichiers et
une autre base qui représente des actions à appliquer sur l'autre coté.

En local, inotify ou équivalent nous fournit des événements du système de
fichiers local. On s'en sert pour remplir la base de données qui représente le
système de fichiers local, et en déduire des actions que l'on rajoute dans
l'autre base.

Pour le cozy distant, le principe est globalement le même, mais la source est
le changes feed des io.cozy.files du Cozy. Attention, nous n'avons pas
d'équivalence entre 1 entrée du changes feed et une action :

- 1 entrée peut correspondre à plusieurs actions (un fichier modifié puis
  renommé dans la foulée va, par exemple, avoir 1 entrée pour 2 actions)
- plusieurs entrées pour une action (un déplacement d'un répertoire avec
  des sous-répertoires)
- et l'ordre peut également être inversé (création d'un répertoire, puis upload
  d'un fichier dans ce répertoire, et déplacement du répertoire).

Quand on ajoute une action dans une base, il faut regarder si l'action peut se
combiner avec une autre action précédente. Par exemple, si un fichier foo1 a
été renommé en foo2, puis que foo2 a été renommé en foo3, on ne va garder
qu'une seule action renommage de foo1 en foo3.

La partie sync suit deux règles importantes :

- on applique d'abord en local les actions qui viennent du cozy distant en cas
- de conflit, on résout le conflit par une action locale, pas une action
  sur le Cozy.

Pour appliquer une action qui vient du cozy distant, il faut la permuter avec
les actions locales. Par exemple, si l'action est l'ajout d'un fichier dans le
répertoire foo sur le Cozy, et que l'on a une action locale pas encore
appliquée sur le Cozy pour renommer le répertoire foo en bar, il faudra
transformer l'action pour créer le fichier en local dans le répertoire bar.

Puis, quand on a fini d'appliquer une action, il faut regarder s'il faut
modifier les actions locales par rapport à l'action qui vient d'etre appliquée.
Par exemple, si un fichier a été ajouté en local dans le répertoire foo et que
l'on applique un renommage de foo en bar (qui vient du cozy), il faut modifier
l'action pour dire d'uploader le fichier dans le répertoire bar.

## Mon avis

Je suis assez sceptique sur ce mode de synchronisation, qui me parait assez
fragile pour les raisons suivantes :

1. Ca me fait fortement penser à
[OT](https://en.wikipedia.org/wiki/Operational_transformation), qui est connu
pour être une horreur à implémenter correctement avec tous les cas particuliers
(et je ne pense pas que l'on puisse une prendre une bibliothèque toute faite et
l'utiliser directement dans notre cas).

2. Les mécanismes anti boucles me semblent compliqués à implémenter (quand un
fichier est ajouté sur le cozy, on va appliquer cette action sur le FS locale,
ce qui va créer des événements inotify, et donc mettre à jour le FS local et
rajouter une nouvelle action, mais si le fichier distant est modifié avant que
cette action soit appliquée, il ne faut pas créer de conflit).

3. Les mécanismes de rejeu en cas d'erreur me semblent également compliqués à
implémenter. Cela vient du fait que l'on ne peut pas simplement mettre en fin
de liste une action, il faut la permuter avec les autres actions dans la file
d'attente (par exemple, si l'action qui échoue est l'ajout d'un fichier dans un
répertoire foo, et que l'action suivante consiste à renommer foo en bar, il
faudra mettre à la fin de la liste une action pour ajouter le fichier dans le
répertoire bar), et il n'est pas toujours possible de permuter les actions (on
ne peut pas mettre la création d'un répertoire après l'ajout d'un fichier dans
ce répertoire).

4. Attention aux race conditions : événements du FS local qui remontent juste
après une action distante qui a été appliquée par exemple.

5. J'ai un doute sur la capacité à relancer le client desktop proprement après
un crash (transactions ACID sur les bases de données ?).

Globalement, j'ai le sentiment que cette proposition a un peu trop de points
communs avec les faiblesses connues du client desktop actuelles : être capable
d'interpréter correctement le changes feed de CouchDB en une série d'actions
dans le bon ordre par exemple, ou la probable nécessité d'avoir des verrous
assez larges autour des accès aux bases de données.

Enfin, ce mécanisme de synchronisation risque de rendre difficile l'ajout de
futures fonctionnalités telles que la synchronisation sélective, au lieu de les
simplifier (on va ajouter de nouvelles combinaisons telles que fusionner une
action de déplacement d'un fichier depuis un répertoire synchronisé vers un
répertoire non-synchronisé, suivie d'une autre action de déplacement vers un
répertoire synchronisé).
