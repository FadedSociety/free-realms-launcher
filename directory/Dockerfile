# Free Realms Expanded realm directory: one small process, list kept in memory (see directory.py).
FROM python:3.12-slim
COPY requirements.txt /app/requirements.txt
RUN pip install --no-cache-dir -r /app/requirements.txt
COPY directory.py /app/directory.py
ENV PYTHONUNBUFFERED=1
USER nobody
CMD ["python", "/app/directory.py"]
