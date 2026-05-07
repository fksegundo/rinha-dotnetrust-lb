# rinha-dotnetrust-lb

## PT-BR

Repositorio separado do load balancer usado pela submissao `rinha-dotnetrust`.

Objetivo:

- manter a imagem do gateway sob nosso controle
- desacoplar a submissao principal do codigo do balancer
- publicar uma imagem propria no Docker Hub para ser usada no `docker-compose.yml` final

Stack:

- Rust
- monoio + io_uring
- proxy TCP simples para duas instancias da API via Unix Domain Socket

Build local:

```bash
docker build \
  -t filonsegundo/rinha-dotnetrust-lb:submission \
  .
```

Publicar:

```bash
LB_IMAGE=filonsegundo/rinha-dotnetrust-lb:submission \
./scripts/publish-image.sh
```

## EN

Standalone repository for the load balancer used by the `rinha-dotnetrust` submission.

Goals:

- keep the gateway image under our control
- decouple the main submission repository from balancer source code
- publish our own Docker Hub image for the final `docker-compose.yml`

Stack:

- Rust
- monoio + io_uring
- simple TCP proxy to two API instances over Unix Domain Sockets

Local build:

```bash
docker build \
  -t filonsegundo/rinha-dotnetrust-lb:submission \
  .
```

Publish:

```bash
LB_IMAGE=filonsegundo/rinha-dotnetrust-lb:submission \
./scripts/publish-image.sh
```

## Variables

- `RUST_TARGET_CPU` - default: `haswell`
- `LB_IMAGE` - default: `filonsegundo/rinha-dotnetrust-lb:submission`
