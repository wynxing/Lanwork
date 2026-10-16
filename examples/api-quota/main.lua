local function show_error(message)
  ui.text("summary", message)
  ui.progress("used", 0)
end

local function apply(body)
  local data = json.decode(body)
  if type(data) ~= "table" or type(data.used) ~= "number" or type(data.limit) ~= "number" or data.limit <= 0 then
    show_error("额度读取失败")
    return
  end
  local used = data.used
  if used < 0 then
    used = 0
  end
  if used > data.limit then
    used = data.limit
  end
  ui.text("summary", string.format("已用 %g / %g", used, data.limit))
  ui.progress("used", used / data.limit)
end

local function refresh()
  http.request({
    method = "GET",
    url = "https://api.example.com/v1/quota",
    secret = { id = "api_key", header = "Authorization", prefix = "Bearer " },
  }, function(result)
    if not result.ok then
      show_error(result.error or "额度读取失败")
      return
    end
    apply(result.body)
  end)
end

function on_load()
  refresh()
  timer.every(300, refresh)
end
