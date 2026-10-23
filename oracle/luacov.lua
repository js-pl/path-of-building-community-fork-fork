-- luacov config for the oracle, selected with LUACOV_CONFIG.
-- Output goes to $COVERAGE_DIR so nothing is written into the submodule.
local dir = assert(os.getenv("COVERAGE_DIR"), "COVERAGE_DIR is not set")
return {
	statsfile = dir .. "/luacov.stats.out",
	reportfile = dir .. "/luacov.report.out",
	include = { "^Modules/Calc[^/]*$", "^Modules/ModParser$" },
	-- Also report files the corpus never loads
	includeuntestedfiles = true,
	savestepsize = 1000000,
}
