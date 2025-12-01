From the current code vs. p1-secrets.md, the main gaps are:

- **Vault effects / rotation path:** We added enums/schemas, but there’s no runtime support or example harness for vault.put/vault.rotate, nor a design-time rotation flow that emits a manifest patch.
- **Receipt redaction & secret_meta:** Injection happens, but receipts/journal don’t yet carry secret_meta, and redaction is left to adapters; there’s no kernel-level enforcement or adapter implementations beyond the HTTP/LLM mocks.
- **Strict resolver requirement in examples:** The LLM demo still relies on a demo key resolver; there’s no real resolver configuration story beyond that.
- (DONE) **Normalization choice:** We’re tolerating multiple SecretRef variant shapes instead of canonicalizing params before hashing/dispatch (spec allows one canonical form).
- **Docs/examples for rotation and resolver config:** No example/readme showing how to rotate secrets via vault.* or how to configure resolvers outside the demo key.

Everything else in the spec (manifest secrets, SecretRef schemas, policy ACLs, injection-only v1 stance) is implemented.