# cluster-view

The first Penelope app: a view of the running clusters and their parts.

It is a built-in view, not HTML. The manifest names `"view": "clusters"`; the app reads the state files under
`.penelope/clusters/*.json` through the app gate, so it can read only inside `.penelope` and has no network and no
shell. Each line carries the cluster id, its state and its progress, followed by its parts.

Install it from this checkout:

```sh
penelope app install apps/cluster-view
penelope app on cluster-view
```

In the interface, `/app install apps/cluster-view` shows the check result and the rights and waits for a yes. Then
`/app` or F7 opens the launcher; Enter shows the cluster view in the interface.

The manifest format is:

```json
{
  "name": "cluster-view",
  "version": "1.0.0",
  "description": "The running clusters and their parts, read from .penelope/clusters.",
  "view": "clusters",
  "rights": {"files": true, "network": false, "shell": false}
}
```

An HTML app uses `"entry": "index.html"` instead of `"view"`. The entry must stay inside the app folder; `..`, an
absolute path and a link that leaves the folder are refused. The optional `"source"` records where the app came from.
