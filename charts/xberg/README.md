# Xberg Helm chart

Deploy the [Xberg](https://github.com/xberg-io/xberg) document-intelligence server
(HTTP API + MCP) on Kubernetes. Extract text, tables, metadata, and structured data
from PDFs, Office documents, images, and 98 formats, with optional OCR.

The chart is published as an OCI artifact to GHCR.

## Install

```bash
helm install xberg oci://ghcr.io/xberg-io/charts/xberg --version 1.0.0-rc.25
```

Override values inline or with a file:

```bash
helm install xberg oci://ghcr.io/xberg-io/charts/xberg \
  --version 1.0.0-rc.25 \
  --set service.type=LoadBalancer \
  --set cache.size=5Gi
```

## Configuration

Values are validated against `values.schema.json`. Common keys:

| Key                  | Default            | Description                                              |
| -------------------- | ------------------ | ------------------------------------------------------- |
| `replicaCount`       | `1`                | Replicas. Keep at 1 with ReadWriteOnce cache storage.   |
| `image.repository`   | `xberg-io/xberg`   | Image repository (`ghcr.io` registry).                  |
| `image.tag`          | `""`               | Defaults to the chart `appVersion`.                     |
| `service.type`       | `ClusterIP`        | Service type.                                            |
| `service.port`       | `80`               | Service port.                                            |
| `ingress.enabled`     | `false`            | Enable Ingress.                                          |
| `ingress.extraLabels` | `{}`               | Extra labels on the Ingress resource.                    |
| `ingress.annotations` | `{}`               | Annotations on the Ingress resource.                     |
| `resources`          | 512Mi/500m → 2Gi/2 | Container requests and limits.                           |
| `autoscaling.enabled`| `false`            | Enable a HorizontalPodAutoscaler.                        |
| `xberg.logLevel`     | `info`             | Log level: trace, debug, info, warn, error.             |
| `xberg.ocrLanguage`  | `eng`              | Default OCR language.                                    |
| `cache.enabled`      | `true`             | Persist downloaded models on a PVC (90 MB–1.2 GB each). |
| `cache.size`         | `2Gi`              | Cache PVC size.                                          |

See [`values.yaml`](values.yaml) for the full, commented list.

## Cache

Embedding, OCR, and layout models range from 90 MB to 1.2 GB and are re-downloaded
on every pod restart without a persistent cache. `cache.enabled=true` (the default)
mounts a PVC to keep them across restarts. With `ReadWriteOnce` storage, keep
`replicaCount: 1` and `strategy.type: Recreate` to avoid Multi-Attach errors.

## Documentation

Full deployment guide: <https://docs.xberg.io/guides/kubernetes/>

## License

MIT — see the [Xberg repository](https://github.com/xberg-io/xberg).
