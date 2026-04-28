# Changelog

## [0.4.0](https://github.com/chrisfentiman/creft/compare/creft-v0.3.4...creft-v0.4.0) (2026-04-28)


### ⚠ BREAKING CHANGES

* **runner:** creft_exit drain + CREFT_ARG_ prefix + drop exit 99 + skill-test polish ([#80](https://github.com/chrisfentiman/creft/issues/80))
* skill test framework + add test + remove test ([#77](https://github.com/chrisfentiman/creft/issues/77))

### Features

* **alias:** namespace aliases for skill paths ([#82](https://github.com/chrisfentiman/creft/issues/82)) ([ab3058f](https://github.com/chrisfentiman/creft/commit/ab3058f4cb0ae0c8104385f728ed6d2e23a18882))
* **search:** hand-rolled XOR filter search primitive with fuzzy matching ([#66](https://github.com/chrisfentiman/creft/issues/66)) ([5d738a3](https://github.com/chrisfentiman/creft/commit/5d738a3f27142d694084f457fc611a68b1de2a29))
* skill test framework + add test + remove test ([#77](https://github.com/chrisfentiman/creft/issues/77)) ([bf903d8](https://github.com/chrisfentiman/creft/commit/bf903d84293e232e7740345f0bf60610b790f744))
* **store:** redb-backed searchable key-value store primitive ([#67](https://github.com/chrisfentiman/creft/issues/67)) ([6404028](https://github.com/chrisfentiman/creft/commit/6404028e1ac9c67c61425d41c697897c397fcbd3))


### Bug Fixes

* **preamble:** drain Node stdout before process.exit ([#64](https://github.com/chrisfentiman/creft/issues/64)) ([2465632](https://github.com/chrisfentiman/creft/commit/24656328d4200112958a964b1341ec6e27027c94))
* **runner:** creft_exit drain + CREFT_ARG_ prefix + drop exit 99 + skill-test polish ([#80](https://github.com/chrisfentiman/creft/issues/80)) ([b56b249](https://github.com/chrisfentiman/creft/commit/b56b249cf61d562facdb3627592bd600e085a7b8))

## [0.3.4](https://github.com/chrisfentiman/creft/compare/creft-v0.3.3...creft-v0.3.4) (2026-04-17)


### Features

* **setup:** default to global install, dynamic session start ([#62](https://github.com/chrisfentiman/creft/issues/62)) ([1933245](https://github.com/chrisfentiman/creft/commit/19332458e13dd26c3dfd2d85326a0ac79c0d558c))


### Bug Fixes

* **welcome:** fall back to 256-color when truecolor is unavailable ([#60](https://github.com/chrisfentiman/creft/issues/60)) ([0bb43cd](https://github.com/chrisfentiman/creft/commit/0bb43cda424289c83272c68f9cafaf3771ba6d94))

## [0.3.3](https://github.com/chrisfentiman/creft/compare/creft-v0.3.2...creft-v0.3.3) (2026-04-16)


### Bug Fixes

* **markdown:** detect and prevent nested fence parsing bugs ([#58](https://github.com/chrisfentiman/creft/issues/58)) ([e970391](https://github.com/chrisfentiman/creft/commit/e970391ceb9bcfcc442d7367c0cce2187473782b))

## [0.3.2](https://github.com/chrisfentiman/creft/compare/creft-v0.3.1...creft-v0.3.2) (2026-04-16)


### Bug Fixes

* **release:** overwrite existing assets on re-run ([#56](https://github.com/chrisfentiman/creft/issues/56)) ([72b71d1](https://github.com/chrisfentiman/creft/commit/72b71d1cbefa34a7790ec43dc8232a3c38016dd2))

## [0.3.1](https://github.com/chrisfentiman/creft/compare/creft-v0.3.0...creft-v0.3.1) (2026-04-15)


### Bug Fixes

* **runner:** print error message when a skill exits non-zero ([#52](https://github.com/chrisfentiman/creft/issues/52)) ([9b57f4a](https://github.com/chrisfentiman/creft/commit/9b57f4a09c87751c4913923681bd38612d527b43))
* **substitute:** empty string defaults resolve to empty, not '' ([#51](https://github.com/chrisfentiman/creft/issues/51)) ([0eaec1d](https://github.com/chrisfentiman/creft/commit/0eaec1d2c9ec4a5f8880bf3ea90d1f161f184dcd))

## [0.3.0](https://github.com/chrisfentiman/creft/compare/creft-v0.2.8...creft-v0.3.0) (2026-04-15)


### ⚠ BREAKING CHANGES

* runtime bindings, shell completions, and install infra

### Features

* runtime bindings, shell completions, and install infra ([1c97273](https://github.com/chrisfentiman/creft/commit/1c9727377ec18235d6804dd0008cbdae7b986ee8))

## [0.2.8](https://github.com/chrisfentiman/creft/compare/creft-v0.2.7...creft-v0.2.8) (2026-04-12)


### Bug Fixes

* **registry:** skip README.md in command discovery ([#45](https://github.com/chrisfentiman/creft/issues/45)) ([fa9c2cf](https://github.com/chrisfentiman/creft/commit/fa9c2cf9aa0e0cbf570490f80c78f8bf1484ed39))

## [0.2.7](https://github.com/chrisfentiman/creft/compare/creft-v0.2.6...creft-v0.2.7) (2026-04-12)


### Features

* **plugin:** plugin marketplace with install/activate lifecycle ([1a1a1ed](https://github.com/chrisfentiman/creft/commit/1a1a1edc76b84818b6e553fad5c0dba48cda2f56))

## [0.2.6](https://github.com/chrisfentiman/creft/compare/creft-v0.2.5...creft-v0.2.6) (2026-04-11)


### Bug Fixes

* **runner:** sponge-to-sponge cancel and clean Ctrl+C ([8bd18d5](https://github.com/chrisfentiman/creft/commit/8bd18d513cd7a209a6a7e456ff8cf7828945d4a9))

## [0.2.5](https://github.com/chrisfentiman/creft/compare/creft-v0.2.4...creft-v0.2.5) (2026-04-10)


### Features

* **runner:** capture child stderr to suppress ANSI noise ([c284552](https://github.com/chrisfentiman/creft/commit/c2845520895c7e1f79198c9225329866848c2ae6))

## [0.2.4](https://github.com/chrisfentiman/creft/compare/creft-v0.2.3...creft-v0.2.4) (2026-04-10)


### Bug Fixes

* **runner:** eliminate flaky exit-99 pipe tests ([f722c64](https://github.com/chrisfentiman/creft/commit/f722c64aedf18d3436c2b7c1fb57368110fe3f32))

## [0.2.3](https://github.com/chrisfentiman/creft/compare/creft-v0.2.2...creft-v0.2.3) (2026-04-10)


### Features

* **list:** hidden commands with _-prefix convention ([c3e973e](https://github.com/chrisfentiman/creft/commit/c3e973e463321f75c5a7bf102caec7d7ebf9ec46))

## [0.2.2](https://github.com/chrisfentiman/creft/compare/creft-v0.2.1...creft-v0.2.2) (2026-04-10)


### Bug Fixes

* **router:** longest-match resolution, sponge cancel race, test cleanup ([cbc8d24](https://github.com/chrisfentiman/creft/commit/cbc8d24d2069ac7c04e0864b417521a14170ef01))

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
