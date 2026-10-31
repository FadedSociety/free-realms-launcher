# Realm directory

The server browser's list of public realms. Hosts' launchers announce a listed realm about once a minute;
every launcher reads `GET /v1/servers`. The list lives in memory, so moving the directory anywhere loses
nothing: listed realms re-announce within a minute.

Trust: an announce is signed by the realm's listing key, and before listing a realm the directory asks its
login broker (AUTH2 type 3, server v0.1.5+) to sign a fresh nonce with that key. Only public addresses
are contacted. Tests: `python -m unittest test_directory`.

## Run it anywhere

    docker build -t osfr-directory .
    docker run -p 8080:8080 -e DIRECTORY_PROXY_HOPS=0 osfr-directory

`DIRECTORY_PROXY_HOPS` is how many proxies append to `X-Forwarded-For` in front of it (1 on Cloud Run,
0 when exposed directly). Run one instance: the list is per process.

## Google Cloud Run (current host)

    gcloud run deploy osfr-directory --source . --region us-central1 --allow-unauthenticated \
      --max-instances 1 --memory 256Mi --set-env-vars DIRECTORY_PROXY_HOPS=1

To move hosts, deploy the same image elsewhere and change the launcher's directory URL setting.
