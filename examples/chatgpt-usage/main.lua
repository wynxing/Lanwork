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

local function window_name(seconds)
  if seconds == 18000 then
    return "5小时"
  end
  if seconds == 604800 then
    return "每周"
  end
  if seconds > 0 and seconds % 86400 == 0 then
    return string.format("%d天", seconds / 86400)
  end
  if seconds > 0 and seconds % 3600 == 0 then
    return string.format("%d小时", seconds / 3600)
  end
  return "窗口"
end

local function reset_ms(value)
  if type(value) ~= "number" then
    return nil
  end
  if value >= 1000000000000 then
    return value
  end
  if value >= 1000000000 then
    return value * 1000
  end
  return nil
end

local function show(text_id, bar_id, percent, seconds, reset, plan)
  if type(percent) ~= "number" then
    ui.text(text_id, "没有数据")
    ui.progress(bar_id, 0)
    return
  end
  local name = "窗口"
  if type(seconds) == "number" then
    name = window_name(seconds)
  end
  local when = ""
  local ms = reset_ms(reset)
  if ms ~= nil then
    when = " · " .. format_utc(ms)
  end
  local label = string.format("%s %g%%%s", name, percent, when)
  if type(plan) == "string" and plan ~= "" then
    label = plan .. " · " .. label
  end
  ui.text(text_id, label)
  local ratio = percent / 100
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
  login.fetch("chatgpt", function(result)
    if not result.ok then
      show_error(result.error or "请求失败")
      return
    end
    local values = result.values or {}
    show("five", "five_bar", values.five_percent, values.five_seconds, values.five_reset, values.plan)
    show("week", "week_bar", values.week_percent, values.week_seconds, values.week_reset, nil)
  end)
end

function on_load()
  refresh()
  timer.every(300, refresh)
end
