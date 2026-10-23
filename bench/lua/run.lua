-- Phase 0 baseline: times upstream node and anoint power single-threaded and
-- with parallel worker processes, and checks both give identical results.
--
-- usage: luajit run.lua --workers N --out DIR [--tasks LIST] BUILD...
--
-- Must run with the working directory set to upstream/PathOfBuilding/src;
-- use bench/run.sh.

local benchDir = arg[0]:match("^(.*)/[^/]+$") or "."
package.path = benchDir .. "/?.lua;" .. package.path

local pob = require("pob")
local dkjson = require("dkjson")

local opts = { workers = 4, tasks = "node:combined,node:FullDPS,anoint", builds = { } }
do
	local i = 1
	while i <= #arg do
		local name = arg[i]:match("^%-%-(.+)$")
		if name then
			opts[name] = assert(arg[i + 1], "missing value for --" .. name)
			i = i + 2
		else
			table.insert(opts.builds, arg[i])
			i = i + 1
		end
	end
	opts.workers = tonumber(opts.workers)
	assert(opts.out and #opts.builds > 0, "--out and at least one build are required")
end

local function shellQuote(text)
	return "'" .. text:gsub("'", "'\\''") .. "'"
end

local function workerCommand(buildPath, task, stat, extra, outFile)
	return table.concat({
		"env -u CI luajit", shellQuote(benchDir .. "/worker.lua"),
		"--build", shellQuote(buildPath), "--task", task, "--stat", stat, extra,
		"--out", shellQuote(outFile), "2>", shellQuote(outFile .. ".err"),
	}, " ")
end

-- Starts all commands at once and waits for all of them
local function runParallel(commands)
	local t0 = pob.clock()
	local procs = { }
	for _, cmd in ipairs(commands) do
		table.insert(procs, assert(io.popen(cmd, "r")))
	end
	for _, proc in ipairs(procs) do
		proc:read("*a")
		proc:close()
	end
	return pob.clock() - t0
end

local function parseResult(file)
	local result = { meta = { }, jobs = { }, count = 0 }
	local f = io.open(file, "r")
	if not f then
		error("worker produced no output: " .. pob.read_file(file .. ".err"))
	end
	for line in f:lines() do
		local kind, key, value, fingerprint = line:match("^(%a+)\t([^\t]*)\t([^\t]*)\t?(.*)$")
		if kind == "meta" then
			result.meta[key] = tonumber(value) or value
		elseif kind == "job" then
			assert(not result.jobs[key], "duplicate job " .. key)
			result.jobs[key] = { ms = tonumber(value), fingerprint = fingerprint }
			result.count = result.count + 1
		end
	end
	f:close()
	local err = pob.read_file(file .. ".err")
	if not result.meta.calc_ms and not result.meta.skipped then
		error("worker failed:\n" .. err)
	end
	return result
end

-- Merges worker results; returns the merged result and per worker metas
local function mergeResults(results)
	local merged = { jobs = { }, count = 0 }
	for _, result in ipairs(results) do
		for key, job in pairs(result.jobs) do
			assert(not merged.jobs[key], "job ran in two workers: " .. key)
			merged.jobs[key] = job
			merged.count = merged.count + 1
		end
	end
	return merged
end

local function compare(baseline, parallel)
	local mismatches = { }
	for key, job in pairs(baseline.jobs) do
		local other = parallel.jobs[key]
		if not other then
			table.insert(mismatches, "missing in parallel run: " .. key)
		elseif other.fingerprint ~= job.fingerprint then
			table.insert(mismatches, "different result: " .. key)
		end
	end
	for key in pairs(parallel.jobs) do
		if not baseline.jobs[key] then
			table.insert(mismatches, "extra in parallel run: " .. key)
		end
	end
	table.sort(mismatches)
	return mismatches
end

local function sumMs(result)
	local total = 0
	for _, job in pairs(result.jobs) do
		total = total + job.ms
	end
	return total
end

os.execute("mkdir -p " .. shellQuote(opts.out))
local report = { workers = opts.workers, runs = { } }

local function log(fmt, ...)
	io.stderr:write(string.format(fmt, ...), "\n")
end

for _, buildPath in ipairs(opts.builds) do
	local buildName = buildPath:match("([^/]+)$"):gsub("%.%w+$", "")
	for taskSpec in opts.tasks:gmatch("[^,]+") do
		local task, stat = taskSpec:match("^(%a+):?(.*)$")
		if stat == "" then
			stat = "combined"
		end
		local prefix = opts.out .. "/" .. buildName:gsub("[^%w]", "_") .. "-" .. taskSpec:gsub("[^%w]", "_")
		log("%s / %s: single-threaded upstream...", buildName, taskSpec)

		local baselineFile = prefix .. "-baseline.tsv"
		local baselineExtra = task == "node" and "--upstream" or "--shard 1 --shards 1"
		local baselineWall = runParallel({ workerCommand(buildPath, task, stat, baselineExtra, baselineFile) })
		local baseline = parseResult(baselineFile)

		if baseline.meta.skipped then
			log("  skipped: %s", baseline.meta.skipped)
		else
			log("  %d calcs in %.1f s; %d workers...", baseline.count, baseline.meta.calc_ms / 1000, opts.workers)
			local commands, files = { }, { }
			for shard = 1, opts.workers do
				local file = prefix .. "-worker" .. shard .. ".tsv"
				table.insert(files, file)
				table.insert(commands, workerCommand(buildPath, task, stat,
					string.format("--shard %d --shards %d", shard, opts.workers), file))
			end
			local parallelWall = runParallel(commands)
			local workerResults = { }
			local maxWorkMs, maxStartupMs = 0, 0
			for _, file in ipairs(files) do
				local result = parseResult(file)
				table.insert(workerResults, result)
				maxWorkMs = math.max(maxWorkMs, result.meta.plan_ms + result.meta.calc_ms)
				maxStartupMs = math.max(maxStartupMs, result.meta.boot_ms + result.meta.load_ms)
			end
			local parallel = mergeResults(workerResults)
			local mismatches = compare(baseline, parallel)

			local run = {
				build = buildName,
				task = task,
				stat = stat,
				calcs = baseline.count,
				startup_ms = baseline.meta.boot_ms + baseline.meta.load_ms,
				baseline_calc_ms = baseline.meta.calc_ms,
				baseline_wall_ms = baselineWall,
				mean_calc_ms = sumMs(baseline) / baseline.count,
				parallel_work_ms = maxWorkMs,
				parallel_startup_ms = maxStartupMs,
				parallel_wall_ms = parallelWall,
				speedup_work = baseline.meta.calc_ms / maxWorkMs,
				speedup_wall = baselineWall / parallelWall,
				mismatches = #mismatches,
				mismatch_examples = { unpack(mismatches, 1, math.min(#mismatches, 10)) },
			}
			table.insert(report.runs, run)
			log("  %.1f s with workers (%.2fx); %s", maxWorkMs / 1000, run.speedup_work,
				#mismatches == 0 and "identical results" or (#mismatches .. " MISMATCHES"))
			for _, line in ipairs(run.mismatch_examples) do
				log("    %s", line:sub(1, 200))
			end
		end
	end
end

local f = assert(io.open(opts.out .. "/report.json", "w"))
f:write(dkjson.encode(report, { indent = true }))
f:close()

-- Markdown summary
print(string.format("Workers: %d\n", opts.workers))
print("| Build | Task | Calcs | ms/calc | Single-threaded | Parallel | Speedup | Parallel incl. startup | Speedup | Identical |")
print("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |")
for _, run in ipairs(report.runs) do
	print(string.format("| %s | %s | %d | %.1f | %.1f s | %.1f s | %.2fx | %.1f s | %.2fx | %s |",
		run.build, run.task == "node" and "node power (" .. run.stat .. ")" or "anoint power",
		run.calcs, run.mean_calc_ms, run.baseline_calc_ms / 1000, run.parallel_work_ms / 1000, run.speedup_work,
		run.parallel_wall_ms / 1000, run.speedup_wall, run.mismatches == 0 and "yes" or ("no (" .. run.mismatches .. ")")))
end
