# Multiscan View Demo

This demo prepares a small tree of `.logjet` files and opens `ljx view` across
the whole dataset.

It is meant to show:

- many files scanned as one dataset
- grouped shards such as `small_1.logjet`, `small_2.logjet`, `small_3.logjet`
- larger single files such as `single-big.logjet`
- directory input for `ljx view`
- concat or merge-style dataset ordering

## Build First

From the project root:

```bash
make demo
```

## Run

From this directory:

```bash
./run-demo.sh
```

## What It Does

The script does this:

1. prints `Preparing the demo data` in bright yellow
2. recreates `./logs`
3. creates four dataset subdirectories
4. writes six `.logjet` files into each subdirectory
5. mixes larger `single-big.logjet` files with grouped `small_1.logjet`,
   `small_2.logjet`, `small_3.logjet`, plus extra random shards
6. opens `ljx view` on `./logs` so the full dataset is scanned

## Files

- `./run-demo.sh`
  - prepares the dataset and opens `ljx view`
- `./logs/<group>/*.logjet`
  - demo inputs for multiscan browsing

## Optional Environment

Override the dataset ordering mode:

```bash
DATASET_ORDER=merge-ts ./run-demo.sh
```

Allowed values:

- `concat`
- `merge-seq`
- `merge-ts`
