-- Report: each customer's total order amount.
-- BUG: this references a column that does not exist.
SELECT c.name, SUM(o.total)
FROM customers c
JOIN orders o ON o.customer_id = c.id
GROUP BY c.name;
