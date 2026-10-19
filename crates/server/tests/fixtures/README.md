# Synthetic OAuth test material

`oauth-test-only-rsa.pem` is a deliberately public, disposable RSA private key generated solely for loopback unit tests. Its matching JWKS uses `kid=tokengo-test-only`. It never signs production credentials and must never be trusted outside tests. No user/provider key or OAuth token is included.
