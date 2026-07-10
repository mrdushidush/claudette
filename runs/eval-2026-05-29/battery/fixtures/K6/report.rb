# Decoy: formats a report line, unrelated to the passing? bug.
module Report
  def self.line(name, score)
    "#{name}: #{score}"
  end
end
