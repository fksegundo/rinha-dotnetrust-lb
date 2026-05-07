# rinha-dotnetrust-lb

Repositorio separado do load balancer usado pela submissao `rinha-dotnetrust`.

Objetivo:

- manter a imagem do gateway sob nosso controle
- desacoplar a submissao principal do codigo do balancer
- publicar uma imagem propria no Docker Hub para ser usada no `docker-compose.yml` final

## Stack

- Rust
- monoio + io_uring
- proxy TCP simples para duas instancias da API via Unix Domain Socket

## Build local

```bash
docker build \
  -t fksegundo/rinha-dotnetrust-lb:latest \
  .
```

## Publicar

```bash
LB_IMAGE=fksegundo/rinha-dotnetrust-lb:latest \
./scripts/publish-image.sh
```

## Variaveis

- `RUST_TARGET_CPU` - default: `haswell`
- `LB_IMAGE` - default: `fksegundo/rinha-dotnetrust-lb:latest`
