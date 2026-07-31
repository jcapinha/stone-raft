---
name: local-docker
description: Validates container images locally with docker build and docker run before remote CI or deploy. Use when implementing or reviewing Dockerfiles, docker-compose, container images, or any containerized deployment; when the user asks to test or run a container locally; or when proposing pipeline-only validation.
---

# Local Docker validation

Container changes must be verifiable on the developer machine (WSL/bash) before remote CI, registry push, or environment deploy is treated as the first proof.

## Core rules

1. **Local first**: Prove `docker build` and `docker run` succeed locally unless the user explicitly asks to skip local Docker testing.
2. **Mirror automation**: Use the same Dockerfile path (`-f` when not default), build context, and build-args as whatever build config exists in the repo (CI workflow, Makefile, compose file, deploy script). Discover these from the codebase; do not assume a specific platform or filename.
3. **Concrete commands**: When implementing or reviewing Docker-related changes, include copy-pasteable WSL/bash commands with paths and tags taken from the current repo, not generic placeholders, unless a value is genuinely unknown.
4. **Run to verify**: After build, run the image to check entrypoint/CMD, logs, and exit code. Prefer `docker run --rm` for one-off checks.

## When implementing

After Dockerfile or container-related code changes:

1. Locate the Dockerfile and how the repo builds it (search for `docker build`, compose `build:`, CI jobs, scripts).
2. Align local commands with that definition: `-f`, context directory, `--target`, `--build-arg`.
3. Propose and run (or ask the user to run) local validation from the correct working directory:

```bash
docker build -f <dockerfile-path> -t <local-tag> <build-context>
docker run --rm <local-tag>
```

4. Add flags the container needs at runtime (`-e`, `-p`, `-v`, `--network`, config mounts). Do not assume secrets or env vars; read them from docs, compose, or deploy config in the repo.
5. Interpret results: build failures, missing context files, wrong `WORKDIR`/`COPY`, crash on start, non-zero exit, missing env.
6. Only after local build/run passes (or the user explicitly skips) discuss pushing to a registry, CI pipelines, or deploy to target environments.

## When reviewing

For changes that touch Dockerfiles, image build config, or container runtime config:

- [ ] Local `docker build` matches how the repo/CI builds (`-f`, context, args)
- [ ] `docker run` (or documented flags) exercises the real entrypoint
- [ ] `COPY`/`ADD` paths are valid from the stated build context
- [ ] Base image tag is pinned or justified
- [ ] Reviewer can reproduce without triggering remote CI first

If the change lacks local commands, add them in review feedback or in the implementation response.

## Aligning with CI (any platform)

1. Find the canonical build invocation in the repo (workflow YAML, Jenkinsfile, `Makefile`, `docker compose`, shell scripts).
2. Translate to local `docker build` with the same `-f`, context path, `--target`, and `--build-arg` values.
3. Use any local image tag; registry hostnames from CI are not required for local validation.
4. If local and CI definitions disagree, flag the mismatch before relying on either path.

## What not to do

- Do not treat remote CI, registry builds, or deploy pipelines as the first validation step by default.
- Do not suggest "merge and let CI build it" without local `docker build`/`docker run` unless the user asked to skip local testing.
- Do not assume a specific cloud provider, CI product, or repo layout.
- Do not use a build context that differs from the repo's documented/automated build without calling out the mismatch.
- Do not omit `-f` when the Dockerfile is not `Dockerfile` at the context root.

## Reporting results

After running or reasoning about local Docker:

- **Build**: success/failure and relevant error lines
- **Run**: command used, exit code, and whether logs look healthy
- **Next step**: fix locally, or proceed to CI/deploy only when local checks pass or were explicitly skipped
