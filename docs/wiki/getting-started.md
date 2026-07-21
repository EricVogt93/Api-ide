# Getting started

## Create a project

1. Start Forge and choose **File → New Project**.
2. Select an empty directory and accept the generated defaults.
3. Add a folder for a story, then choose **+ Request**.
4. Select an environment and run the request.

Forge derives asset paths from the selected project node. You do not configure separate save locations for requests, assertions, hooks or environments.

## Open the demo

Open `examples/demo-workspace` to see requests, sidecars, reusable assets, environments, auth refresh, OpenAPI coverage, mocks, sequences and generated-test inputs. Start with `requests/pets/list.request.json`, then use **View → User tour**.

## First useful workflow

1. Add an OpenAPI file under `specs/` or set one on a project/folder in Properties.
2. Use the OpenAPI sidebar to add an operation or generate a valid value.
3. Configure assertions in the **Assertions** tab and hooks in **Hooks**.
4. Run once, inspect **Response**, then save.
5. Commit the ordinary files with Git.
