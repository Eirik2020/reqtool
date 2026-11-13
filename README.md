# reqtool

Requirements and verification tracing for Rust projects. Requirements are YAML, each deriving from one of the
project's roots or from another requirement. Source files declare the requirements
they implement. Evidence records the hash of what it covered, and status is
computed from it, never written.

## The rules

- **A car has its own requirements and sets requirements on its parts.** A first
  pass verifies the car. A part replaced by one that fulfils the same requirements
  owes only its own evidence, so wheels can change without reverifying the car.
  Changing what a requirement sets on the layers below re-owes that requirement.
- **A file is the unit.** It states its requirements (`//! Requirements: A-001,
  B-002`, a line ending in a comma continuing on the next `//!` line) and is
  hashed, ignoring whitespace and comments. Evidence records the hash, and a mismatch flags it.
- **Dependencies are pinned.** Bumping one flags the files importing it until a
  maintainer has reviewed a diff analysis, or they are reverified. Contributors may
  do the analysis ([Reviewed bumps](#reviewed-bumps)).
- **The compiler is not part of evidence.** Differences between compiler versions
  are negligible outside safety assurance, which reqtool does not address.
- **The hazard decides who runs a test.** An agent may run and record anything
  with no hazard. A person runs whatever is dangerous or could damage expensive
  equipment, and a pass counts only where its preconditions held.
- **A library presents its requirements and supplies its evidence.** A project
  using it maps its own requirements onto them, by crate and ID
  (`my-hal:UART-012`), and inherits that evidence. IDs carry no project
  prefix; the crate name does that.

## A project's settings

`reqtool.toml` at a project's root marks it and says how it is laid out:

```toml
name = "my-project"                         # the name others link by
requirements = "requirements"               # requirement YAML, at any depth
roots = "requirements/goals.md"             # lines like `1. **G1 - Title.**`
root_prefix = "G"
evidence = "requirements/evidence/runs"
# Optional:
gates = "requirements/gates.yaml"           # the default, beside requirements
artifacts = "logs"                          # where a run's files on hardware sit
boards = "firmware"                         # one firmware crate per board
retired = ["archive"]                       # no active procedure may point in here
critical_metadata = "reqtool"               # [package.metadata.<this>] critical = true
every_file_traced = true                    # every file owes tracing, with no marker
every_file_traced_except = ["hil"]          # what it leaves out, by path prefix
test_features = [["chip-a"], ["chip-b"]]    # host tests, per feature set
reviews = "requirements/reviews"            # the default, beside requirements
maintainers = ["a-maintainer"]              # who may review a diff analysis
```

`every_file_traced` makes every source file safety-critical, for a project with
no per-file notion of it. `every_file_traced_except` leaves paths out of that,
such as bench crates beside a library. Each entry is a path prefix relative to
the repository, matched by whole components. A file is left out only when the
real file, links resolved, sits inside the repository under an entry, and so
does every package whose module tree reaches it: a file the library compiles in
through `#[path]` stays traced wherever it sits. A file left out is
safety-critical only if its module doc says so, and then every rule applies to
it. Leaving a file out changes only whether the tracing rules and `report` count
it as safety-critical, never what evidence covers or any status; `report` lists
apart the requirements that only left-out files it compiles in declare. Every
command refuses an entry that is empty, absolute, a pattern, uses `\`, has an
empty, `.` or `..` component, repeats another or overlaps it, or leaves out no
source file, and the list without `every_file_traced`. `validate` and `check`
state what each entry leaves out.

## Commands

Run from anywhere inside the repository, or pass `--repository`:

```text
cargo run --release -- validate
cargo run --release -- check
cargo run --release -- check INPUT-001
cargo run --release -- check my_crate::rc_receiver
cargo run --release -- check crates/my-crate/src/rc_receiver.rs
cargo run --release -- report --board my-board
cargo run --release -- review --crate my-hal --from 0.1.0 --to 0.1.1 \
    --analysis docs/my-hal-0.1.1.md --analysed-by a-contributor --reviewed-by a-maintainer
```

- `validate` exits nonzero on any problem: schema, IDs, references, tests
  that do not resolve or are not `#[test]`, missing procedure headings, and
  declarations of unknown requirements. It also checks every
  safety-critical file: one that defines code declares a requirement
  and it imports only safety-critical files and external crates its manifest
  pins to one version, which is freedom from interference. It lints requirement text.
- `check` with no target summarizes every requirement. With an ID it shows
  that requirement and the files that declare it; with a root the
  requirements derived from it; with a file, by path or by module path, the
  requirements it declares. Agents run `check` on a file before changing it.
  An entry whose status stands on a reviewed bump says so, naming each step.
- `report --board <board> [--features a,b]` reports maturity for one
  configuration: every requirement the safety-critical files it
  compiles in declare, with its status there, and the safety-critical files
  that declare nothing. It runs `cargo check` for the board to find what is
  compiled in.
- `test` runs the host tests that requirements name (all, or those of the
  requirements given) with cargo, in each package's own workspace, and
  records the session. Tests that fail or do not run exit nonzero; passes
  already on record with the same coverage are not recorded again.
- `gate` lists the project's gates;
  `gate <name> --board <board>` says whether one is open and lists every
  entry in its way. It exits zero only when the gate is open. A gate names
  requirements (an ID, a prefix ending in `*`, or `*`) and the levels they
  must be verified at, and may include other gates.
- `record` writes one run of a test or procedure: a result, at the current
  code, for every requirement entry that names that method. Evidence on hardware
  (`target`, `system` and `field`) needs `--board`. `--failed REQ` fails one
  requirement in a run that otherwise passed; `--dry-run` prints the record.
  `--details FILE` adds how the run was done (JSON): `execution`
  (`performed_by`, `user_confirmed`), `candidate` (`firmware_sha256`,
  `features`, ...), `conditions` (any `name: state`, plus `equipment` and
  `environment`), `commands`, `artifacts` (paths under the artifact directory;
  `reqtool` hashes them), `measurements`, `observations`, `limitations` and
  `stop_conditions_triggered`. Evidence on hardware must have it, with
  `execution`; the image (`candidate`) is optional, and its SHA-256 only
  checked for form when given. An entry with a hazard must be run by a person who confirms
  it. A pass needs each of its entry's preconditions stated as held, and may not
  include a triggered stop condition.

```text
cargo run --release -- record --test my_crate::rc_receiver::tests::name --result pass
cargo run --release -- record --procedure "procedures.md#Heading" --board my-board --result pass
```

- `review` writes a maintainer's review of a diff analysis of one dependency
  bump into the reviews directory, refusing what `validate` would refuse and
  never overwriting a review. It prints the requirements whose status it
  changes.

## Status

| Status | Meaning |
|---|---|
| `verified` | a pass whose coverage matches the current code |
| `stale` | a pass, but what it covered has changed since |
| `unverified` | no pass at all |
| `failed` | a fail whose coverage matches; a later pass with nothing changed does not clear it |

A fail clears only when the code changes and a run passes, or when a record
`--supersedes` it with the user's `--ruling`. Board evidence is reported per
board; an entry takes the worst board with evidence.

Every result carries its claim: the requirement's ID and statement
(whitespace-insensitive), the statements of everything derived from it, the entry,
and a procedure's own section.

- **Host evidence** covers the modules a run exercised: a test's file and the
  files it imports directly. A source file's identity ignores whitespace and
  comments; an external crate's is its locked version. A host pass counts for a
  file that declares its requirement while the claim, the file and the external
  crates it imports are unchanged; another local file is a part with its own
  evidence. A pass that reached no declaring file, or a requirement no file
  declares, takes what the run exercised. An entry stands on a pass that counts
  for any file, and a requirement's files are covered together: each file a host
  test build compiles (`cargo check --workspace --tests`) needs a pass that counts
  for it, or the requirement's host entries fall to `stale`, or to `unverified` if
  no test ever reached it. A file that build does not compile, such as a task
  definition, is judged by target-level evidence alone. `check` names both kinds
  of file. The compiler is not part of evidence.
- **Board evidence follows files, not images.** A board record carries the
  configuration it ran in: which repository files `cargo check` reports compiled
  in, each with its identity, and the external crate versions. A board result
  counts while every file declaring its requirement that the run compiled in is
  unchanged, and every external crate such a file imports is at the version it
  ran with (or a reviewed bump of it). Nothing else in the image is part of it:
  a file added beside them, the bench's own code, another run's code, the build
  profile or a rebuilt binary leave it standing, so a UART verified on a bench
  keeps its evidence when an LED driver joins the board. A run that compiled in
  no declaring file stands while every file it compiled in is unchanged.
- **Every declaring file needs covering.** A requirement's board entries stand
  only while each file declaring it is covered by a counting pass: a host test
  that exercised it, or a board run that compiled it in. A file only another
  chip compiles (a second IP version, another chip's glue) owes its own run,
  and `check` names it until one passes.

Records are JSON, schema 4, in the project's evidence directory: one
record per session, each result naming its own method. Schema 4 records
ending `__retraced` were derived from schema 3 ones at their own revisions,
and list what could not be derived.

### Reviewed bumps

A dependency bump flags evidence that recorded the old version. Rather than
reverify, someone writes an analysis of what the bump changed, and a maintainer
reviews it. A review names one crate and one step, `from` to `to`, and evidence
recorded with that crate at `from` then counts as if it were at `to`, for host
and board evidence alike.

- It is directional and chains: `A` to `B` and `B` to `C` carry evidence at `A`
  to `C`, never `C` to `A`.
- It bridges only that crate's version. A changed file, a changed claim, another
  crate moving, or any other difference in what evidence covered still stales
  it. A file that names the pin, such as a board app's manifest, is a changed
  file.
- A recorded fail is carried forward like a pass, so a review never clears one.
- A version is written as evidence records it: `0.1.0` for a registry crate,
  and `<version>+<commit>` for a git dependency pinned by `rev`, the commit the
  lock file holds. A pin that names an abbreviated commit is recorded on a
  board with that abbreviation, so pin the full commit for one review to cover
  both. Records from before both sides agreed hold `<version>+rev=<commit>`,
  which compares as the same version.

A review is one JSON file in the reviews directory: `schema_version`, `crate`,
`from`, `to`, `analysis` (a repository path), `analysis_sha256`, `analysed_by`,
`reviewed_by` and `reviewed_at_utc` (`YYYY-MM-DDTHH:MM:SSZ`). Anyone may write
the analysis; only one of the project's `maintainers` may review it, so a project
with none has no valid reviews. `validate` refuses a review whose reviewer is not
a maintainer, whose analysis is missing, has changed since, or is not a regular
file inside the repository once links are resolved, whose time is not UTC in that
form, or whose `from` and `to` are the same version; and while one is refused no
command runs. The reviews directory must be inside the repository and apart from
the evidence directory.

## A requirement

```yaml
schema_version: 2
requirements:
  - id: INPUT-001                 # <AREA>-<NNN>, stable, never reused
    title: Reject stale RC input
    statement: >-
      Command Input shall treat RC frames older than the link's limit as
      invalid and signal loss of link to the safety master.
    rationale: Stale input must never be acted on as a live command.
    derived_from: [G2]            # a root or parent requirement IDs
    verification:
      entries:
        - level: unit             # analysis | unit | simulation | target | system | field
          environment: host       # host | development | product
          method:
            test: my_crate::rc_receiver::tests::name
        - level: system
          environment: product
          hazard: equipment       # none (the default) | equipment | personnel
          preconditions:          # needed with a hazard; a pass must show them
            propellers: removed
          method:
            procedure: procedures.md#Heading
```

A third method inherits a library's evidence:

```yaml
        - level: target
          environment: development
          method:
            library: my-hal:UART-012
```

Cargo says which copy of `my-hal` this project builds, so the status is what that
revision's own requirements and evidence give, computed there. The crate needs a
`reqtool.toml` with that name, and the requirement needs an entry at the level
claimed. Building it to judge it uses `target/reqtool-libraries/`. An entry like
this covers only the library's part: a requirement's other entries still stand on
this project's own evidence. A pin's commit is part of what evidence records, so
moving it flags the files importing the crate until a review covers the move.

A requirement is verified by its own entries only; a parent is never proved
through its children. Unknown fields are refused, and there is no `status`
field. A requirement with no entries is valid: nothing verifies it yet, and
`check` reports it as a gap.

The code that implements it says so in its module doc comment, and a
safety-critical file says that too:

```rust
//! Requirements: INPUT-001
//! Safety-critical.
```

Paths resolve through modules, `use` re-exports and globs, plus one segment
after a type for an enum variant or a method in an `impl` in the type's own
module. An import that does not resolve to one file widens: a glob to every
file it re-exports, a macro to the files defining it, a type or trait to the
files implementing it. Paths inside macro invocations are read from their
tokens. Items made by macros are invisible; `#[cfg]` is ignored, and a module
declared once per `#[cfg]` resolves through whichever declaration holds the item.
