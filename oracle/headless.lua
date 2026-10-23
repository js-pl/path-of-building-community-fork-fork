-- Starts upstream headless and loads builds. Shared by dump.lua and bench.lua.
-- Must run with upstream's src/ as the working directory.

local headless = { }

-- Parses "<id>=<path>" arguments
function headless.parseBuildArgs(args)
	local list = { }
	for _, a in ipairs(args) do
		local id, path = a:match("^([^=]+)=(.+)$")
		assert(id, "expected <id>=<path>, got " .. a)
		list[#list + 1] = { id = id, path = path }
	end
	assert(#list > 0, "no builds given")
	return list
end

-- useModCache: desktop PoB loads the ModCache, while upstream skips it
-- whenever the CI environment variable is set and parses every mod line live
function headless.start(useModCache)
	-- Upstream's Main treats arg[1] as a build link to download
	arg = { }
	-- HeadlessWrapper waits for input on startup errors
	io.read = function() return nil end
	local getenv = os.getenv
	os.getenv = function(name)
		if name == "CI" then
			return not useModCache and "1" or nil
		end
		return getenv(name)
	end
	dofile("HeadlessWrapper.lua")
	os.getenv = getenv
	assert(build, "upstream failed to start")
end

function headless.readFile(path)
	local f = assert(io.open(path, "rb"))
	local text = f:read("*a")
	f:close()
	return text
end

-- Loads a PoB XML (.xml) or PoE API character JSON (.json) build
function headless.loadBuild(id, path)
	if path:match("%.json$") then
		-- HeadlessWrapper's loadBuildFromJSON expects the character fields at
		-- the top level; the API response nests them under "character".
		-- Same import calls as upstream's TestImport_spec.
		local data = require("dkjson").decode(headless.readFile(path))
		data = data.character or data
		newBuild()
		build.importTab:ImportPassiveTreeAndJewels(data, true)
		build.importTab:ImportItemsAndSkills(data)
		runCallback("OnFrame")
	else
		loadBuildFromXML(headless.readFile(path), id)
	end
	assert(build.calcsTab.mainEnv, "build did not calculate: " .. path)
end

return headless
