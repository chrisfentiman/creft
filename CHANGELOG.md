# Changelog

## [0.2.1](https://github.com/chrisfentiman/creft/compare/creft-v0.2.0...creft-v0.2.1) (2026-04-02)


### Features

* **runner:** RunContext, module split, BlockRunner trait, unified exit 99 ([8e33674](https://github.com/chrisfentiman/creft/commit/8e33674d1405e565445f14762b27f17352ab6cae))

## [0.2.0](https://github.com/chrisfentiman/creft/compare/creft-v0.1.9...creft-v0.2.0) (2026-04-01)


### ⚠ BREAKING CHANGES

* pipe-by-default — multi-block skills always use concurrent pipes

### Features

* pipe-by-default — multi-block skills always use concurrent pipes ([9df2369](https://github.com/chrisfentiman/creft/commit/9df2369261af10b62ea49d547b52fc92cdd10bc0))

## [0.1.9](https://github.com/chrisfentiman/creft/compare/creft-v0.1.8...creft-v0.1.9) (2026-04-01)


### Features

* **runner:** LLM sponge pattern, buffered relay, and pipe exit 99 fixes ([ae626f1](https://github.com/chrisfentiman/creft/commit/ae626f11d927aee822b4eb71c26838b819a38865))

## [0.1.8](https://github.com/chrisfentiman/creft/compare/creft-v0.1.7...creft-v0.1.8) (2026-04-01)


### Bug Fixes

* **runner:** kill remaining pipe children on exit 99 ([16879f2](https://github.com/chrisfentiman/creft/commit/16879f27587372f2e703f215c8899bfdecc6c911))

## [0.1.7](https://github.com/chrisfentiman/creft/compare/creft-v0.1.6...creft-v0.1.7) (2026-04-01)


### Bug Fixes

* **runner:** leave unmatched template placeholders as literal text ([a71ea97](https://github.com/chrisfentiman/creft/commit/a71ea97d9b2ced8dd9822f8fb5be47996e6e7ba5))

## [0.1.6](https://github.com/chrisfentiman/creft/compare/creft-v0.1.5...creft-v0.1.6) (2026-04-01)


### Features

* **runner:** native llm block type with provider-agnostic AI CLI support ([ab515cd](https://github.com/chrisfentiman/creft/commit/ab515cd125e0c4fb23573b11a6ca00f9fc22cfca))

## [0.1.5](https://github.com/chrisfentiman/creft/compare/creft-v0.1.4...creft-v0.1.5) (2026-04-01)


### Features

* **runner:** exit 99 as early successful return in block pipelines ([e31868f](https://github.com/chrisfentiman/creft/commit/e31868f8436af0929bc79b84e2561d90dcbca7ed))

## [0.1.4](https://github.com/chrisfentiman/creft/compare/creft-v0.1.3...creft-v0.1.4) (2026-04-01)


### Bug Fixes

* **runner:** use npm install + NODE_PATH for node block deps ([382164a](https://github.com/chrisfentiman/creft/commit/382164a8c83f296f2f09c9861b88b6e9973b2f85))

## [0.1.3](https://github.com/chrisfentiman/creft/compare/creft-v0.1.2...creft-v0.1.3) (2026-03-31)


### Features

* **runner:** add --verbose flag and fix optional arg validation ([5380cf9](https://github.com/chrisfentiman/creft/commit/5380cf94a15b641b1e79b0a39c2d04f61574d50f))

## [0.1.2](https://github.com/chrisfentiman/creft/compare/creft-v0.1.1...creft-v0.1.2) (2026-03-31)


### Bug Fixes

* **validate:** optional arg defaults, shellcheck, and command checker ([88bacc2](https://github.com/chrisfentiman/creft/commit/88bacc25dc56193f023186ce4a86c6bb997c2dc6))

## [0.1.1](https://github.com/chrisfentiman/creft/compare/creft-v0.1.0...creft-v0.1.1) (2026-03-31)


### Features

* **creft:** open source creft ([4d96c11](https://github.com/chrisfentiman/creft/commit/4d96c11d242337dcf381951444a89b9f29121be5))


### Bug Fixes

* **ci:** extract Homebrew formula template to fix YAML parsing ([c7a2dd8](https://github.com/chrisfentiman/creft/commit/c7a2dd874262537ae5a6eed16a4caeb026f6a118))
* **model:** exclude global ~/.creft/ from local root walk-up ([f1a0634](https://github.com/chrisfentiman/creft/commit/f1a063411774ec54973b6a83fa3292bca5eb64d0))
* **runner:** two-step cast for signal handler pointer ([2d742e7](https://github.com/chrisfentiman/creft/commit/2d742e773931e05491e9850454c5829cfa9b1165))
