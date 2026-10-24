-- Exact, deterministic JSON for oracle outputs. See oracle/README.md.

-- JSON encoding. Exact and deterministic:
-- - finite numbers use %.17g, which round-trips every double,
-- - non-finite numbers are {"$num": "inf" | "-inf" | "nan"},
-- - tables with keys 1..n are arrays, others are objects with sorted keys,
--   an empty table is {},
-- - fields in objectRefs point at game objects instead of holding results;
--   they are replaced by the name upstream itself shows for them,
-- - anything else (functions, cycles, class instances) fails loudly.
local encode
-- Replacement for function values, set per call
local funcValue

-- Same identities as the warnFunc of the Req* stats in BuildDisplayStats.lua
local objectRefs = {
	sourceGem = function(gem) return { ["$gem"] = gem.nameSpec } end,
	sourceItem = function(item) return { ["$item"] = item.name } end,
}

local function encodeString(s)
	return '"' .. s:gsub('[%c"\\]', function(c)
		if c == '"' then return '\\"'
		elseif c == "\\" then return "\\\\"
		elseif c == "\n" then return "\\n"
		elseif c == "\t" then return "\\t"
		else return string.format("\\u%04x", c:byte()) end
	end) .. '"'
end

local function encodeNumber(n)
	if n ~= n then
		return '{"$num":"nan"}'
	elseif n == math.huge then
		return '{"$num":"inf"}'
	elseif n == -math.huge then
		return '{"$num":"-inf"}'
	end
	return string.format("%.17g", n)
end

local function isArray(t)
	local n = 0
	for _ in pairs(t) do
		n = n + 1
	end
	if n == 0 then
		return false
	end
	for i = 1, n do
		if t[i] == nil then
			return false
		end
	end
	return true
end

local function encodeTable(t, path, stack, out)
	if stack[t] then
		error("cycle at " .. path)
	end
	if getmetatable(t) ~= nil then
		error("table with metatable at " .. path)
	end
	stack[t] = true
	if isArray(t) then
		out[#out + 1] = "["
		for i, v in ipairs(t) do
			if i > 1 then out[#out + 1] = "," end
			encode(v, path .. "[" .. i .. "]", stack, out)
		end
		out[#out + 1] = "]"
	else
		local keys = { }
		for k in pairs(t) do
			local kt = type(k)
			if kt ~= "string" and kt ~= "number" then
				error("key of type " .. kt .. " at " .. path)
			end
			keys[#keys + 1] = { str = tostring(k), key = k }
		end
		table.sort(keys, function(a, b) return a.str < b.str end)
		for i = 2, #keys do
			if keys[i].str == keys[i - 1].str then
				error("duplicate key " .. keys[i].str .. " at " .. path)
			end
		end
		out[#out + 1] = "{"
		for i, k in ipairs(keys) do
			if i > 1 then out[#out + 1] = "," end
			out[#out + 1] = encodeString(k.str)
			out[#out + 1] = ":"
			local v = t[k.key]
			if objectRefs[k.key] and type(v) == "table" then
				v = objectRefs[k.key](v)
			end
			encode(v, path .. "." .. k.str, stack, out)
		end
		out[#out + 1] = "}"
	end
	stack[t] = nil
end

function encode(v, path, stack, out)
	local vt = type(v)
	if vt == "table" then
		encodeTable(v, path, stack, out)
	elseif vt == "number" then
		out[#out + 1] = encodeNumber(v)
	elseif vt == "string" then
		out[#out + 1] = encodeString(v)
	elseif vt == "boolean" then
		out[#out + 1] = tostring(v)
	elseif vt == "function" and funcValue then
		encode(funcValue(v), path, stack, out)
	else
		error("value of type " .. vt .. " at " .. path)
	end
end

-- funcValue: optional replacement for function values; without it they fail
return function(v, replaceFunc)
	local out = { }
	funcValue = replaceFunc
	encode(v, "$", { }, out)
	funcValue = nil
	return table.concat(out)
end

