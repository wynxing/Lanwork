function on_load()
  ui.text("status", "准备")
end

function on_click(id)
  if id ~= "start" then
    return
  end
  ui.timer("clock", 60)
  ui.timer_running("clock", true)
  ui.text("status", "倒数")
end

function on_timer(id)
  if id ~= "clock" then
    return
  end
  ui.timer_running("clock", false)
  ui.text("status", "到了")
end
