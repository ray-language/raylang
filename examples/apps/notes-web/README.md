# Notes (website with a React frontend)

Notes as a website: a React + TypeScript frontend built with Vite and embedded in the binary,
a JSON API with the `web` framework, and the notes in Redis. It is the project of the handbook
chapter [Site with a React frontend](../../../handbook/web-react.en.md).

```sh
npm --prefix frontend install
ray dev                                            # Vite with hot reload + the program
NOTES_TEST_REDIS=1 REDIS_PORT=… ray test           # the Redis store
ray build --native --release                       # one binary: page, assets and API
```
