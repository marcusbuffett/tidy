#!/usr/bin/env node
import { run } from "./lib/commands";
import { camelCase } from "lodash";
import { readFileSync } from "node:fs";
import unlisted from "unlisted-pkg";
import { gone } from "./lib/does-not-exist";
run(camelCase(readFileSync("x", "utf8")), unlisted, gone);
