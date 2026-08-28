# Changelog

All notable changes to Sonduit are documented here.

This project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
and its commits follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/).

Changelogs are generated for minor and major releases only. Patch releases
carry fixes and chores; their notes point at the commit history. See
docs/adr/ADR-008-versioning.md.

## 2.0.0 - 2026-08-27


### Bug Fixes

- **ci**: Correct three defects in the version tooling ([`a15edf80`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/a15edf808d8daf1df3ece2db5ab1e4f8a24dd992))
- **core**: Repair the jitter buffer and close the defects review found ([`1901401b`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/1901401b649e39572dd172dac64fd888d0a46a94))
- **tools**: Sync the version onto the workspace dependency requirements ([`dde0d84c`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/dde0d84c187e0b00b48dbc1d18718fcfd9651a34))
- **ci**: Stamp develop builds without breaking workspace resolution ([`9377ad87`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/9377ad8738884bffe5032acb6a96f402a5771fc9))
- **ci**: Repair the three failures in the first green-field run ([`38047403`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/380474030c6ae3b49be4a0b966c0ec7804fd5c0b))
- **ci**: Mark the gradle wrapper executable ([`8143ec00`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/8143ec007e5ef1c06c7c43a94d8eb3b24ad8ab98))
- **ci**: Give the rolling develop tag a name that is not a branch ([`aef5c104`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/aef5c104c723e946b58fa71bc8da7d6bb5c28745))
- **desktop**: Ship the FFmpeg licence and show the notice ([`17dde25f`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/17dde25fa04c60f809625fb14aa361799540cf8b))
- **desktop**: Rebuild the editor as two scrolling columns ([`5d34d794`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/5d34d794e79fc4c5d22e40b5a2f38c5786ee31e5))
- **desktop**: Restore the accents stripped from five locales ([`d12dc916`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/d12dc91693539a2ef5638f986753d652e6ab4e5c))

### Build System

- **ci**: Add language, commit and version tooling ([`9f316eda`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/9f316eda2273d59aa8dcce96cc44c6d260a86171))
- **tools**: Reject a bad commit message before it is written ([`cdf131f7`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/cdf131f76cb2778fecc85e7bbc529832c035d7a0))

### Continuous Integration

- Add the build, develop, release and release-pr workflows ([`43e76d3a`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/43e76d3a15bd658cc44b7ce5a467a85107212b18))

### Documentation

- **protocol**: Document the scream wire format and licence boundaries ([`96a6b54d`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/96a6b54dc72e84e8399b8bc86a89a703cef97a49))
- Add the architecture decisions, research and latency budget ([`2c2cd222`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/2c2cd222e454977e4a247599e31ee8b8b1aa2774))

### Features

- **desktop**: Rebrand to sonduit and adopt the soft acrylic shell ([`9f79b47f`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/9f79b47f4d5ff5586509f04ef9c6d26acd69a9a3))
- **desktop**: Replace native form controls with themed components ([`d6621f6a`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/d6621f6af61ed1da4639b496e8b83fb6f679135f))
- **desktop**: Move titlebar chrome to a menu cluster ([`b2299569`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/b2299569385df8d74238fec05905f2cc12935677))
- **core**: Add the cargo workspace, shared core and walking skeleton ([`dcd56418`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/dcd5641860f44aa72239d89c93a1897b1a80d777))
- **desktop**: Restore the audio editor screens ([`96ce94bb`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/96ce94bbb8efb111580f9513534ec16a0f974bd8))
- **desktop**: Implement the audio processing backend ([`3f8bd75f`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/3f8bd75fc373accb18f045f85417f14bcb83482d))
- **capture**: Implement WASAPI loopback capture ([`8e074db9`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/8e074db9e69a0727c23ffb8a72826550ef6d7cea))
- **desktop**: Wire the capture path into the shell ([`9570b0b8`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/9570b0b877624111b66bce3464a8ecce5f7f0b0a))
- **android**: Build the receiver app ([`228bdcd9`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/228bdcd9f5bc35b07e88d9a034280a383df53852))
- **core**: Correct clock drift instead of only measuring it ([`ad7f23a5`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/ad7f23a5a2c768af06a2bdde784208972b40e155))
- **transport**: Require pairing before a device can be selected ([`0de22de7`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/0de22de7f61bda2ef7e88a8f82bbaf2df1753943))
- Size the buffer for the link and find a tethered phone without asking ([`37ca57ac`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/37ca57ac225a806d4974862d6b4f12d6dca572e5))
- **desktop**: Reopen the capture device instead of ending the session ([`397a4069`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/397a40693c449894f3112f405b97d151343ef76e))
- **android**: Translate the app into the eleven languages the desktop has ([`25933916`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/25933916252bd1f64a60cec24f4d39c79c0d6899))
- **core**: Stop the buffer target chasing the jitter estimate ([`03cd0fd7`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/03cd0fd79a665fd502b1432a684af98163b43017))
- **desktop**: Measure before mastering, when the measurement fits ([`722f09a9`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/722f09a967cfd1c903f3deff52cfd7de9c6fc88d))
- **transport**: Make the receiver report, and stop inventing telemetry ([`a5647e1b`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/a5647e1b5a648b4678feb91c82ec194a49d3b681))
- Pair by QR, and lay the connection page out in two columns ([`bd4b3acd`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/bd4b3acda2d6e991de59675c7808a4a7db72c933))

### Refactoring

- **desktop**: Split the editor into three tabs ([`7423b17e`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/7423b17e958db406ac3745e62595ff2dd69e491f))

### Chore

- Set the version baseline to the last shipped release ([`81202e98`](https://github.com/h1dr0nn/sonduit-audio-bridge/commit/81202e98b7df399f64f7d6064b1cdd5c491a1367))

