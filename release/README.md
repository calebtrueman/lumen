# Release signing certificate

`lumen.cer` is the public certificate of the key that signs Lumen releases.
PCs that approved Lumen under Secure Boot trust exactly this certificate, so
every release must be signed with its private key (`keys/lumen.key`, kept
offline and never committed). `tools/dist.sh` refuses to build a release
signed with any other key.

SHA-256 fingerprint:
`DA:CB:F2:AD:3E:36:36:52:47:30:D5:CE:3D:02:BF:CB:16:5D:ED:9E:C1:D9:D6:DD:BF:C0:4A:3C:47:C7:FC:81`

If the private key is ever lost, new releases need a new key, and every PC
must approve it once more. If it leaks, anything signed with it would boot
on PCs that trust it: remove it from each PC with
`mokutil --delete lumen.cer` and issue a new key.
