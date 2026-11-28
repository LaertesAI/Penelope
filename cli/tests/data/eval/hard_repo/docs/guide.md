# Guide

The colour of a status badge shows the state of a job. Version 1.4.2 added the grey colour.

## Install

Copy the folder and run the report:

```sh
# the colour flag is a real flag of the tool, keep it
python3 -m app.cli all --colour never
```

## Colours

Pick a colour per region. The colour table lives in config/settings.json.

```json
{"colour": "red"}
```

## Report format

Each line holds a region and its revenue. The colour of the line follows the region.
