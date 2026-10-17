local function format_utc(ms)
  local secs = math.floor(ms / 1000)
  local days = math.floor(secs / 86400)
  local sod = secs - days * 86400
  local hour = math.floor(sod / 3600)
  local minute = math.floor((sod % 3600) / 60)
  local z = days + 719468
  local era = math.floor(z / 146097)
  local doe = z - era * 146097
  local yoe = math.floor((doe - math.floor(doe / 1460) + math.floor(doe / 36524) - math.floor(doe / 146096)) / 365)
  local doy = doe - (365 * yoe + math.floor(yoe / 4) - math.floor(yoe / 100))
  local mp = math.floor((5 * doy + 2) / 153)
  local day = doy - math.floor((153 * mp + 2) / 5) + 1
  local month = mp + 3
  if month > 12 then
    month = month - 12
  end
  return string.format("%02d-%02d %02d:%02d", month, day, hour, minute)
end

local function find_window(windows, name)
  if type(windows) ~= "table" then
    return nil
  end
  for _, window in ipairs(windows) do
    if type(window) == "table" and window.name == name then
      return window
    end
  end
  return nil
end

local function show(text_id, bar_id, window)
  if type(window) ~= "table" or type(window.used_percent) ~= "number" then
    ui.text(text_id, "没有数据")
    ui.progress(bar_id, 0)
    return
  end
  local when = ""
  if type(window.resets_at_ms) == "number" then
    when = " · " .. format_utc(window.resets_at_ms)
  end
  ui.text(text_id, string.format("%s %g%%%s", window.name, window.used_percent, when))
  local ratio = window.used_percent / 100
  if ratio < 0 then
    ratio = 0
  end
  if ratio > 1 then
    ratio = 1
  end
  ui.progress(bar_id, ratio)
end

local function show_error(message)
  ui.text("five", message)
  ui.text("week", "")
  ui.progress("five_bar", 0)
  ui.progress("week_bar", 0)
end

local function refresh()
  usage.fetch("chatgpt", function(result)
    if not result.ok then
      show_error(result.error or "请求失败")
      return
    end
    show("five", "five_bar", find_window(result.windows, "5小时"))
    show("week", "week_bar", find_window(result.windows, "每周"))
  end)
end

function on_load()
  refresh()
  timer.every(300, refresh)
end
