# Vox
A performant static site generator built to scale.

## Features
* Fast build times.
* Intelligent rebuilding; only pages needing rebuilding are rebuilt.
* Flexible data model, perfect for any static site.

## Overview
Sites have the following structure: 
- A `global.toml` file. Anything here defines the `{{ global }}` context.
- `.vox` files:
    - Inside the `layouts` folder, defining pages which other pages can use as a template; layout pages provide the `{{ layout }}` context to their child pages.
    - Inside the `snippets` folder, defining partial pages which can be embedded in other pages.
    - Inside any other subdirectory, defining pages inside a collection; a collection provides its own context, referred to by the name of the subdirectory.
    - Inside the root folder, defining pages not inside a collection.
All of the items above are optional; even the `global.toml` file is optional if no page requires it.

Building occurs in the following stages:
1. A directed acyclic graph (DAG) of pages (`.vox` files) is built.
2. Pages are built iteratively, descending the hierarchy of pages.
3. If serving, changed pages trigger rebuilds of themselves and their child pages.
    - This avoids the need for a site-wide rebuild.