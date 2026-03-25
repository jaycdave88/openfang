FROM python:3.12-slim

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY a2a_base/ /app/a2a_base/
COPY deerflow/main.py /app/main.py

EXPOSE 8100
CMD ["python", "main.py"]
