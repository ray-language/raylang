# Notes (web API)

Notes as a JSON API with the `web` framework, the notes in Postgres through a connection pool
shared by every request, bearer-token authentication, gzip, JSON logs and graceful shutdown. It is
the project of the handbook chapter [Web API](../../../handbook/api.en.md).

```sh
ray test                                      # validation and auth; the database tests skip
NOTES_TEST_PG=1 PGHOST=… PGPORT=… PGUSER=… PGPASSWORD=… PGDATABASE=… ray test
NOTES_API_TOKEN=… PG…=… ray run              # http://127.0.0.1:8080
```
