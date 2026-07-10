# Grades student scores.
module Grader
  # A score of 60 or above is a passing grade.
  def self.passing?(score)
    score > 60
  end

  def self.letter(score)
    return "A" if score >= 90
    return "B" if score >= 80
    return "C" if score >= 70
    return "D" if score >= 60
    "F"
  end
end
