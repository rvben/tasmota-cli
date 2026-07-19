# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).





## [0.2.0](https://github.com/rvben/tasmota-cli/compare/v0.1.3...v0.2.0) - 2026-07-19

### Breaking Changes

- **cli**: async tokio runtime over the async core ([78b017a](https://github.com/rvben/tasmota-cli/commit/78b017a41f349469f71601d1e43483af5a5ecbe2))
- **core**: async reqwest transport, ops, and switchkit adapter ([50a52f7](https://github.com/rvben/tasmota-cli/commit/50a52f798c8cd1f8c617b617e3e8402efc19bcfa))

### Added

- **cli**: async tokio runtime over the async core ([78b017a](https://github.com/rvben/tasmota-cli/commit/78b017a41f349469f71601d1e43483af5a5ecbe2))
- **core**: async reqwest transport, ops, and switchkit adapter ([50a52f7](https://github.com/rvben/tasmota-cli/commit/50a52f798c8cd1f8c617b617e3e8402efc19bcfa))

## [0.1.3](https://github.com/rvben/tasmota-cli/compare/v0.1.2...v0.1.3) - 2026-07-19

### Added

- **core**: implement switchkit::SmartDevice for Tasmota ([944c48b](https://github.com/rvben/tasmota-cli/commit/944c48b0abbb34acacbc49f1f0412dae967ab928))

## [0.1.2](https://github.com/rvben/tasmota-cli/compare/v0.1.1...v0.1.2) - 2026-07-19

### Fixed

- **core**: anchor credential redaction to query-param boundaries ([6712194](https://github.com/rvben/tasmota-cli/commit/671219487d266887b65e5b5729238856bf0e75e8))
- **core**: redact device credentials from transport errors; bound connect timeout ([1b717d5](https://github.com/rvben/tasmota-cli/commit/1b717d5134613c4a14be32a94116ce8db9d0e0d8))

## [0.1.1](https://github.com/rvben/tasmota-cli/compare/v0.1.0...v0.1.1) - 2026-07-18

### Fixed

- **cli**: UX and robustness fixes from real-hardware testing ([6db45fb](https://github.com/rvben/tasmota-cli/commit/6db45fb159f10d6787e47845869e6f349a760d79))

## [0.1.0] - 2026-07-18

### Added

- tasmota CLI for managing Tasmota devices over HTTP ([649041a](https://github.com/rvben/tasmota-cli/commit/649041acc1e92093762c516b49cf3ad256187cb3))

### Fixed

- **cli**: point crate readme at workspace root README ([fe28a31](https://github.com/rvben/tasmota-cli/commit/fe28a31739c55da39ef21a6ad672aee890e2a2ae))
