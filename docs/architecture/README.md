# Architecture visuals

Diagrams and an animation of the Basic Rollup, drawn in the Amussis visual style.

| File | What it shows |
| --- | --- |
| `system-architecture.svg` | `rollup_client`, the HTTP frontend, the sequencer and its six phases, `RollupDB`, the settlement boundary and the Solana cluster, with which links exist today and which are planned. |
| `transaction-lifecycle.svg` | One v1 transfer through `decode`, `validate`, `load`, `budget`, `process` and `commit`: what each phase does, the Mainnet limits it enforces, and the HTTP status it returns when it refuses. |
| `transaction-lifecycle-animation.html` | The same transfer, animated step by step: the client signing a v1 transaction, submission, each phase in order, the state change, the receipt, the lookup and the settlement boundary. |
| `fonts/inter.woff2` | Inter, the Amussis typeface, with its licence in `fonts/Inter-LICENSE.txt` (SIL Open Font License 1.1). |

## Viewing

Open any of the files in a browser. There is no build step and nothing to install.

```sh
open docs/architecture/transaction-lifecycle-animation.html
```

- The two SVG files follow the system's light or dark setting.
- The animation has an Auto, Light and Dark switch, and also accepts `?theme=light` or `?theme=dark` in the address.
- Animation controls: Play and Pause (Space), Back and Next (arrow keys), Restart (Home), or click any step below the stage. With reduced motion turned on in the system settings, it starts paused and changes step without moving the packet.
- On a narrow screen, the animation's stage scrolls sideways inside its frame.

The files load Inter from `fonts/`. Where a browser refuses fonts from a local file, or where an SVG is shown as an image (as GitHub does), the text falls back to Arial. Serving the folder shows Inter everywhere:

```sh
python3 -m http.server 8765 --bind 127.0.0.1 --directory docs/architecture
```

## Implemented and planned

- Implemented: everything drawn with solid lines, including account pulling. With `ROLLUP_CLUSTER_URL` set, the `load` phase (and `GET /get_account/{address}`) copies each unseen account from the cluster once, with `getMultipleAccounts`.
- Planned, drawn dashed in amber: settlement. Every receipt reports `settlement: "not_configured"`, and nothing is written back to or locked on the cluster. The preferred direction, not yet decided, is a devnet program that takes control of the accounts while the rollup uses them and moves the final lamports between them at settlement. The visuals label it "not decided".
- Dotted: `cargo xtask mainnet` and `cargo xtask live`, which contact a cluster as checks. The server does not run them.

## Accuracy

Labels use the names in `rollup_core/src` and `rollup_client/src`, and every number shown appears in the repository's `README.md` or source. Balances are shown as the changes the tests assert (payer `10000000000 − 100000000 − 5000`, recipient `+100000000`), not as computed totals.

The visuals match the code as of 2026-11-16. When a phase, limit or route changes, update the matching labels in all three files.

## Style

Colours, type and logo come from the Amussis app: the Jade palette for dark mode, its light theme for light mode, Inter, and the Core A logo.
