# Fixtures de test

`test_rsa_private.pem`/`test_rsa_public.pem`/`test_ec_private.pem`/`test_ec_public.pem`:
un par de claves RSA-2048 y un par de claves EC P-256, generadas SOLO para
verificar `crypto.jwtSignRS256`/`crypto.jwtSignES256` (GRAMMAR.md §3.261)
contra una implementación de JWT independiente (PyJWT) durante el
desarrollo de esa feature. No son ni fueron nunca claves reales de ningún
servicio (Google/Apple/etc.) -- generadas localmente con `openssl genrsa`/
`openssl ecparam`, sin ningún uso fuera de este repositorio. Seguras para
vivir en el repo público.
