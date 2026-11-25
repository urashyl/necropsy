The answer is `@typescript_eslint/parser`.

Look at the `devDependencies` in `package.json`. The ESLint plugin is still `@typescript-eslint/eslint-plugin`, but the parser line now says `@typescript_eslint/parser`, with an underscore. npm scopes are just names, and anyone can register `@typescript_eslint`, so it isn't the typescript-eslint project.

The lockfile confirms it. The new package has `"hasInstallScript": true`, so it runs code on every `npm install`, on every developer machine and CI runner. Its only dependency is the real `@typescript-eslint/parser`. That's why lint still works and CI is green: the impostor installs the real parser and runs its own script on the way.

The other install scripts in the diff are fine. `esbuild` downloads its platform binary on install and `fsevents` builds a native module on macOS. Both have done that for years and their names are spelled right. The version bumps are ordinary too. You'd need to compare integrity hashes against the registry to be sure, but nothing about them looks off.

Things that should have made you look twice: a first-time contributor touching dependencies, a "no runtime changes" description on a change that alters what runs at install time, and a lockfile diff too long to read, which is exactly where a one-character change goes unnoticed. Tools like `npm diff`, or a check that flags any new package name in a lockfile, catch this more reliably than reading does.
