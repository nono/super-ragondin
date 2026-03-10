# Interesting links

I have collected some links that can be interesting for making a better
cozy-desktop. Obviously, the dropbox articles on the rewrite of their client
needs to be cited here as they are really aligned with the direction I want to
take for cozy-desktop:

- https://dropbox.tech/infrastructure/rewriting-the-heart-of-our-sync-engine
- https://dropbox.tech/infrastructure/-testing-our-new-sync-engine

## CRDT

A highly-available move operation for replicated trees and distributed
filesystems: [paper](https://martin.kleppmann.com/papers/move-op.pdf).

I'm not convinced by this approach for cozy-desktop, but maybe some ideas can
be taken.

## Simulators

- [FoundationDB](https://www.youtube.com/watch?v=4fFDFbi3toc)
- [Sled](https://sled.rs/simulation.html)

## Languages

I think that a language with static typing is important for cozy-desktop. I
first looked at classical languages like Go, Rust, Haskell, etc. No language
looked like the perfect match, with strong guarantee by the static typing, a
strong ecosystem, and not too hard to learn.

But now, I think it would be better to have a language that transpiles to JS to
help reuse the FS watcher libraries, GUI and packaging stuff of the existing
client, and thus makes the transition easier). I have a bit loosened the
initial constraints by looking at less popular languages and I saw two
languages that could fit:

- [Reason](https://reasonml.org/), an alternative syntax for OCaml that targets
  JS developers
- [Fable](https://fable.io/), the compiler that makes F# a first-class citizen
  of the JavaScript ecosystem.

Prototyping can be a good step before commiting to a language.

## Property based testing

They are not somany good libraries for property based testing (aka QuickCheck),
but I can list:

- [Hedgehog](https://hackage.haskell.org/package/hedgehog) in Haskell
- [Hypothesis](https://hypothesis.works/) in Python
- [Fast-check](https://github.com/dubzzz/fast-check) in JS
- [PropEr](https://github.com/proper-testing/proper) in Erlang
- [Proptest](https://github.com/AltSysrq/proptest) in Rust
- [FsCheck](https://github.com/fscheck/FsCheck) in F#

## Linearizability

[Elle](https://github.com/jepsen-io/elle) (and its predecessor,
[knossos](https://github.com/jepsen-io/knossos)) may be used to check a
linearization property after a run of the simulator.

See also https://www.anishathalye.com/2017/06/04/testing-distributed-systems-for-linearizability/

## Lightway formal specification

- [Alloy](http://alloytools.org/)
- [TLA+](https://lamport.azurewebsites.net/tla/tla.html)

